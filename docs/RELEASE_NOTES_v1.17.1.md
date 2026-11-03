# AutoShade v1.17.1 — the fit closes its loops

Everything merged into `main` since v1.17.0 ships here. This is a maintenance
release of the reverse fit and the sidecar: every item on v1.17.0's
remaining-work list was taken to its cause and either fixed or measured and
recorded. The one idea behind the fixes is that a stage is judged on the
frame that actually ships, not on an estimate taken earlier in the solve.

The design, the rulings and this document are the maintainer's. The code of
this release was written with an AI coding assistant working under the
maintainer's direction, and every change passed the project's own test
battery and reference-pair gate before it shipped.

## The reverse fit

- **The colour-cast curves close their loop on the finished frame.** The
  per-channel curves are read off the finished frame but render before the
  mixer, the grade and the saturation stages, which used to scale what the
  curves delivered. A cast the fit admits now has its knots refined on the
  finished frame, one step at a time while the look improves, and ships only
  if the same checks admit the refined curves. On the neon street pair the
  red fan of the earlier releases is gone and the frame's colour distance to
  the target fell from 5.81 to 4.75. No pair is worse than v1.17.0.
- **Colour bands replace a zone only if the finished frame is no worse.** A
  set of brightness bands that replaces a single zone was held only to the
  zone stage's attachment tolerance, so it could win on the zone's own error
  and leave the whole frame worse than the zone it replaced. Where the
  single zone shipped, the bands may now cost the finished frame nothing
  beyond the mixer check's own margin.
- **The colour-grade wheels obey the strength budget as every stage does.**
  The grade used to move about as far at Strength 0 as at 1. Each wheel is
  now clamped to a ceiling read from the mixer's own band ceiling at that
  strength, and the report says when a wheel is held there.
- **A channel at the clip in both frames testifies to the colour field.** The
  field was solved without clipped pixels but applied to them, so on the
  sunset rooftops pair the upper clouds, a third of whose pixels have red at
  the clip in both frames, were pulled away from their colour. Such pixels
  now hold their vertices; a pixel clipped on one side only still stays out.
  The rooftops pair's colour distance to the target fell from 2.78 to 1.95
  and the seaside street's from 2.28 to 2.08 across this and the fit changes
  above.
- **The zone confidence is read on the render that ships.** The confidence
  verdict was taken before the band trials, so a zone the bands replaced
  still set it from a correction the recipe no longer carried.
- **Replaced zones leave with their notes.** When bands replace a single
  zone, the rationale no longer keeps the replaced zone's lines beside the
  bands that ship.

## The Lightroom sidecar

- **AutoShade's own sidecar reads back as the develop it carries.** A
  sidecar written beside a RAW that already had a Lightroom sidecar keeps
  that sidecar's camera profile, creative Look and lens-correction switch,
  which this writer does not own. On re-import those keys were mistaken for
  Lightroom edits and replaced the develop's values, and the lens correction
  was switched off by the base's switch: the re-import rendered through the
  base's Look with no lens correction, several colour steps away from
  AutoShade's own export. The sidecar now records the document as it was
  written, and the reader measures Lightroom's edits against that record;
  a Lightroom edit made after the write still wins. On three reference pairs
  the re-import is now byte-identical to the export render. Sidecars written
  by earlier versions carry no record and import exactly as before; saving
  the develop again adds it.

## Fixed

- The analyzer and the engine render the colour field in one arithmetic.
  The analyzer rounded the field's parameters through single precision
  before rendering, so its render and the engine's differed by one unit in
  the last place; the documented bit-for-bit property now holds and is
  tested.

## Known limits

- **The night reference pair's horizon band.** About twenty-five rows above
  the night horizon the sky renders a touch cooler than the target, by less
  than one colour step on average and below what the eye separates from the
  sky's own star-and-noise texture at full size. Every limit of the
  colour-field solve was measured on it (smoothness, bounds, iterations, row
  resolution, regularisation) and none moves it at an acceptable price: the
  one lever that does trades it against the fit's own frame objective, which
  weighs brightness far above colour there. Re-weighting that objective is a
  design item for the next release, not a constant to turn.
- **Colour noise reduction** still leaves a faint colour grain that comes from
  the demosaic itself. A smoothing of the colour differences at the sensor
  mosaic's own scale was built and measured for this release: it takes about a
  tenth of the grain off a denoised master and shifts the mean colour of deep
  shadows by up to one colour step, so it did not ship. A mean-preserving form
  is the next release's item.

## Known changes

- **Recipes with colour-cast curves differ from v1.17.0's** where the curves
  now close their loop; the other stages read the finished frame as before.
- **A sidecar grows by a few kilobytes** (the written-document record,
  compressed) when it is written over an existing Lightroom sidecar. A fresh
  sidecar with no base carries no record.

## Compatibility

Recipes and sidecars written by earlier versions open. New sidecar fields:
`asr:Written` and `asr:WrittenCrc32` (AutoShade's own record of the document
as written; ignored by other readers).

## Gates

待填（fill_gates_v1171.py pre 从冻结快照电池与 gate_fit_final_1171 日志生成；目视结论手填）
