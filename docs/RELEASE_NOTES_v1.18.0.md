# AutoShade v1.18.0 — the fit judges what the eye sees

Everything merged into `main` since v1.17.1 ships here. This release changes
how the reverse fit decides whether a correction made the photo closer to its
target. Until now every decision read a distance measured in screen code
values, in which the brightness of a dark sky weighed many times more than
its colour; a correction that the eye reads as better could be scored as
worse and refused. The fit now measures in the colour space the eye's
judgement is modelled on, pixel by pixel wherever the two pictures show the
same thing, and every stage and every control has to earn its place by more
than the fit's own rounding before it ships.

The design, the rulings and this document are the maintainer's. The code of
this release was written with an AI coding assistant working under the
maintainer's direction, and every change passed the project's own test
battery and reference-pair gate before it shipped.

## The reverse fit: how a result is judged

- **Distances are colour distances, measured where they mean something.**
  Where the photo and its target show the same scene in the same place, the
  fit compares them pixel by pixel, on lightly smoothed neighbourhoods so that
  grain and sharpening do not count, as a perceptual colour difference.
  Where they do not correspond — a sky the target repainted, a texture a
  generator invented — it compares the two pictures' distributions of
  lightness and of the two colour axes instead, in the same units, so one
  step of colour costs the same in both readings. The old measure weighed a
  night sky's brightness far above its colour, which is why a correction
  that brought the colour of the sky above the night horizon closer was
  refused in v1.17.1.
- **A region's mode decides how its pixels are read.** When the fit decides
  that one region of the target was repainted rather than re-graded, that
  region is read by its distribution even if the rest of the frame
  corresponds; a region that does correspond inside a repainted frame is read
  pixel by pixel. A correction of the land under a replaced sky is no longer
  charged as a loss because the sky's pixels moved.
- **Every pixel in the picture is counted when a result is judged.** The
  ranges of brightness the fit trusts to estimate a control are a narrower
  set than the picture itself, and the verdicts used to read only those
  ranges: a gain in bright clouds or in a dark foreground could be invisible
  to the decision that admitted it. The estimate still comes from the trusted
  ranges; the verdict now counts every pixel that is present, each the same,
  including those in a region the target replaced.
- **A hue band is judged by how far its colour moved, not only its hue.** The
  term that guards the worst colour band now reads the band's average colour
  offset against the target's own colourfulness there, so a correction that
  turns a muted band garish is charged for it, and a small hue error on a
  nearly grey band is not inflated.
- **Flat areas are no longer read as scrambled.** A clipped sky, a crushed
  shadow or a posterised target holds large areas of one code value. The
  structural comparison ranked those pixels by their position in the picture,
  so two such areas a fraction of a code apart could swap order and read as
  total disorder. Values within one code are now a tie, charged by the
  fraction of a code that separates them.
- **A region's structure is read from that region alone.** The structural
  comparison of a masked region blurred the whole picture at its coarse
  scales, so a region could read as rearranged because of pixels outside it;
  each scale now reads only the region's own pixels, and a detail finer than
  the comparison can resolve reads as absent rather than as a difference.
- **Structure is asked only where the question is structure.** The structural
  comparison still decides how a pair is fitted and compares two whole-frame
  solutions; a local correction (a zone, a band, a tile, the colour field, a
  mask's colour) is judged on colour and tone, and its edges by the seam
  checks that exist for them.

## The reverse fit: what ships

- **A stage ships only if it earns more than the fit's own rounding.** Every
  stage that can be left out — the mixer, the cast curves, the hand-off to the
  white balance, the zones, bands, tiles, masks and the colour field — now
  ships only when the photo with it is closer to the target than the photo
  without it by more than the smallest difference the fit can produce by
  rounding its own controls. A smaller gain is no difference, and no
  difference goes to the simpler recipe; a loss of any size refuses. The
  separate allowances each stage used to carry are gone, so they can no
  longer add up. That rounding is measured, not chosen: an identical pair is
  rendered with every control displaced by half its storage step and read the
  way the fit reads.
- **A local correction is judged on its own pixels.** A zone, a band, a tile
  or a mask ships when it brings its own region closer by that margin; the
  rest of the frame is only a check that it got no worse by more than the
  same margin. Judged on the whole frame, a tile covering a few percent of the
  picture had to be many times better on its own pixels than a whole-frame
  change to count. A zone is now accepted by this one test, in place of four
  older ones that read the earlier measure.
- **Controls that earn nothing are set back to zero.** After each joint solve
  (white balance, tone sliders, curves, mixer bands, grade wheels, cast
  curves) and once more on the finished recipe, each control is set back to
  its neutral value in turn, smallest first, and stays there when the result
  without it is no worse beyond the same rounding. A tone slider that acts on
  one end of the tonal range may earn on that end alone. A target that
  differs from the straight conversion by exposure alone now comes back as
  exposure alone. The report names the controls set back.
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
- **A band too small to be a zone joins its neighbour.** When the fit splits a
  region into brightness bands and one band is smaller than a zone may be, it
  used to refuse the whole split; that band is now merged into the
  neighbouring band and the merged split is tried.
- **A seam is charged to the side that is off.** At the edge between two
  corrected regions, a region that lands on its target is no longer refused
  because the region beside it is still short of its own.
- **The colour field's stop rule compares like with like.** The field's
  ceiling was solved before the zones and compared with the frame after them,
  and it could skip the tiles. The field is now read again on the frame the
  later layers start from, and its ceiling only limits the field and the
  masks carved from it.
- **The report says what ships.** The sentences about a colour cast, its
  hand-off to the white balance and a zone's colour are now derived from the
  finished recipe, so a note never describes a curve or a tint that a later
  step took back, and a refusal prints the reading the decision was made on.
- **A hidden contrast rule is removed.** The tone solve forced contrast to a
  fixed small value whenever the frame was not yet close and the solved
  contrast was near zero. The rule had no recorded reason and no test; the
  solved value now stands.
- **The reported confidence is re-derived.** The confidence a fit reports is
  set so that the one calibration pair whose target is a different photograph
  — which no recipe can match — sits on the confidence floor.

## The reverse fit: the starting point

- **The fit starts from the picture the export would make.** The fit used to
  reduce the photo to its working size first and apply the camera base curve
  to the averages, without the capture sharpening the export applies, so a
  photo compared with the app's own export of the same untouched recipe read
  as different and the fit corrected a gap that was not in the picture. The
  starting picture is now developed exactly as the export develops it,
  resized only after the base's per-pixel steps; an untouched recipe now
  reads as identical to its own export.
- **A JPEG or PNG is read on its own pixels when it is the photo being
  fitted**, the rule the command line already applied to baked files, so the
  calibration's neutral reference can be read through the same entry as
  everything else.
- **The colour field's solve reaches its tolerance.** Its iterative solver is
  now scaled by the system's own diagonal, so on difficult frames it
  converges within the existing step budget instead of stopping on it.
  Renders move by at most one code value.

## Fixed

- **A bright block at a clipped horizon.** Where the global develop clipped
  most of a colour-field cell, the field wrote "no change" into that cell
  beside neighbours that darkened the glow, and the horizon rendered a
  rectangular over-bright block. A cell with too few pixels to measure now
  carries the smooth continuation of its measured neighbours.
- **Light where sky and land meet.** A pixel that is part sky and part land
  now owes each part's light through that part's own correction, instead of
  one blend of the two corrections applied to all of its light.
- **A spacing slip in the colour field's stop note**: the note printed two
  runs of spaces in the middle of its sentence.

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

## Colour noise reduction

待填_PENDING_E3 — the colour noise reduction work (the demosaic grain, a
known limit of v1.17.1) is not final in this tree. When it lands, one bullet
for the mechanism and its known limit. Draft known-limit sentences carried
from the current rulings, to be confirmed or replaced by the committed form:

- (draft, pending) "Colour is averaged with its neighbours only where the
  neighbours are shown to share it: a colour edge is kept where it rises
  above the window's colour noise, and a structure narrower than the smallest
  window keeps its own sample."
- (draft, pending) "At the cell beside a colour edge, a step under about
  待填_S_EDGE colour standard deviations is partly averaged into its
  neighbour; above it the edge is kept."

## How this release was checked

- **Every control the fit can solve has a self-consistency test.** For each
  one — temperature, tint, each colour band's hue, saturation and luminance,
  the grade wheels, the cast curves, a mask's colour — a calibration photo is
  rendered with that control alone set to a known value, the render becomes
  the target, and the fit is asked to find it again from the straight
  conversion: the control must come back with its sign and at least half its
  size, nothing else may move, and the finished frame must land within the
  fit's own rounding. The same test checks that an untouched recipe reads as
  identical to its own export, and that exposure alone comes back as
  exposure alone with the sky clipped and just under the clip.
- The hand-off from the cast curves to the white balance has an end-to-end
  sample that reaches both of its white-balance checks through the public
  entry.

## Known limits

- 待填_SELF_CONSISTENCY — read off the self-consistency test on the final
  tree. At this tree seven cases still fail and are skipped as open defects:
  the white balance, a colour band's hue, saturation and luminance, a cast
  curve, a grade wheel, and a mask's colour. Each is either fixed before the
  release or written here as "a control set alone on a photo is not always
  recovered alone: the fit finds <what it ships instead>".
- **The white balance and exposure judged on the shared part of a repainted
  pair** apply on no calibration pair: on every pair that reaches that path
  the shared part is too small to be used, and the whole frame is read as
  before. The rule is tested on a constructed pair only.
- **One calibration pair is a hair further from its target.** On the lantern
  street the finished frame sits 0.10 ΔE76 further from its target than the
  reading this release is held to, less than the 0.31 ΔE76 the fit's own
  rounding amounts to; its brightest tenth reads 5.94 against that reading's 4.23.
  待填_FINAL_GATE (confirm on the final gate).
- **The sidecar's starting tone is declared, not carried** (above): opened in
  Lightroom, a photo whose recipe starts on the camera base curve renders on
  Lightroom's own starting tone.
- 待填_PENDING_SKY — the night horizon (a known limit of v1.17.1): the sky's
  foot at the horizon stays darker and colder than the target's warm haze on
  the night pair, and the land under it a stop under; the work that
  traces which stage owns the glow is not final in this tree. Final reading
  and sentence from the final gate.

## Known changes

- **Recipes differ from v1.17.1's** on most pairs: the fit judges and admits
  every stage by a different measure, so a fit may now ship fewer controls
  (those that earned nothing are at zero) and different strengths.
- **The report carries new lines**: the controls set back to zero, the
  readings a zone shipped on, and the full-size re-reading of the finished
  recipe beside the preview's and the starting point's.
- **A fit takes one more full-size render** for the final check.

## Compatibility

Recipes and sidecars written by earlier versions open. No new recipe field.
A sidecar AutoShade writes now always states crs:LensProfileEnable when its
recipe carries a lens profile.

## Gates

待填_GATES
