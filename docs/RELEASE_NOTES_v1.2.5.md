# AutoShade v1.2.5 — the frame is measured before the decoder allocates it

Two defects, one report. Opening an upscaler's DNG killed the app with a
third-party panic behind a dialog that said AutoShade had to close. Neither
half of that was true: the panic was caught, and the file was not defective.

## A frame larger than the decoder builds for is refused, not aborted on

The report was a 2× upscale of a 60 MP frame — a 19008×12672 LinearRaw DNG at
three samples per pixel in 416×416 tiles — and it ended in

```
panicked at rawler-0.7.2/src/decoders/mod.rs:598:32:
rawler: surely there's no such thing as a >500MP or >50000 px wide/tall image!
```

AutoShade has had a per-file RAW ceiling since v0.34.0
(`refuse_raw_develop_over_ceiling`: 4 GiB, 138,547,333 px at the measured
31 B/px), and it names the estimate, its per-pixel basis and the fact that
`--jobs 1` is not the answer. It was unreachable here. That ceiling is charged
against the pixel count in a DUMMY `RawImage` — a number only the decoder can
produce — and the decoder has a LOWER ceiling of its own that it PANICS at
(`alloc_image_plain!`, `pixarray.rs:546-556`: `w * h > 500_000_000 ||
w > 50_000 || h > 50_000`), checked BEFORE the `dummy` short-circuit so even
the metadata-only probe trips it. Worse, `plain_image_from_ifd`
(`decoders/mod.rs:598`) charges that ceiling with `decode_width * cpp` as the
WIDTH, so a three-sample linear frame reaches it at a third of the pixel count
a Bayer frame would. For every frame between the two ceilings the probe that
would have measured the file panicked first, and the panic was then reported
as "a defect in the third-party decoder for this format" — which is not what
happened.

`decode::guard_raw_plane_extent` measures the frame from the container
instead, before the decoder is opened. It mirrors the decoder's arithmetic
rather than approximating it: the same tile round-up
(`decoders/mod.rs:588-594`), the same `* cpp` on the width, the same three
comparisons. The reported frame rounds to 19136 tile-columns and asks for
`19136 × 3 = 57408` by `12896` — past both limits at once — and now returns a
sentence that names the frame, the allocation it becomes, that nothing is
wrong with the file, and the workflow that does work: develop the original
frame and run the upscaler on the result.

It sits at all four doors that ask for a `RawImage` — `source_frame`,
`decode_raw_turned`, `render::render_to_image_in` and `render::as_shot_wb` —
and a source scan (`every_raw_image_door_measures_the_plane_first`) fails the
build if a fifth appears without it. It is deliberately NOT at
`raw_orientation`, `embedded_xmp` or `camera_rendition`: those read metadata
and the embedded preview, which an over-ceiling file answers perfectly well,
and refusing there would blank the gallery thumbnail of a photo whose only
problem is that it cannot be developed.

**It rejects only on proof**, the same rule the cyclic-IFD guard follows: any
IO error, any non-TIFF magic, and any IFD that does not declare itself sensor
data (`PhotometricInterpretation` CFA or LinearRaw — a preview or embedded
JPEG is BlackIsZero / RGB / YCbCr) passes through untouched. Measured on the
two files that raised this: the 19008×12672 frame returns the named error, and
the same run's un-upscaled 9504×6336 sibling computes 28704 × 6400 and decodes
as before, as does a 60 MP Bayer ARW at full resolution (9504 × 6336 out).

## The crash dialog stops calling a contained panic fatal

AutoShade catches parser panics in two places — `decode::guard_parser_panic`
around every third-party parser call, and the GUI's `spawn_worker` around
every worker body — and both turn a panic into a message about the one file.
The panic HOOK ran INSIDE both of them and could not tell contained from
fatal, so a single unreadable photo raised "AutoShade hit an internal error
and must close" over an app that kept running and then reported that file's
own error.

`panic_guard` is a thread-local depth, entered by both guards and restored on
the unwinding path (RAII, so the restore survives the panic it describes).
The hook reads it, still writes `panic.log` either way — a contained panic is
still a defect worth recording — and headlines it "recovered from an internal
error" instead of "crashed", without raising a modal. `install_panic_reporter`
already held itself to the rule that the claim must match the outcome; this is
what lets it keep that rule for the contained half.

## Gates

Release battery, this machine: library **1385 passed / 0 failed / 14 ignored**
(1399 enumerated, one process per module), CLI **24**, integration **2 + 2**,
doc-tests 0, GUI **164 passed / 0 failed**; `audit_i18n` 0 findings;
`subset_gui_fonts --check` 875/875 embedded; `check_docs.py` 0 FAIL. By-name
test-set difference against v1.2.4: **+4 / −0**.

**The calibration lane did not run, and is not claimed.** It re-runs the
library with the p36–p39 fit corpus in reach, and that corpus is not on this
machine — it was deleted in a disk clean-up on 2026-09-03. Without it every
corpus-gated test prints a skip line and passes, which is why
`scripts/release_battery.sh` refuses to start at all rather than report a
battery that measured nothing. Its subject is the fit estimator (`src/fit.rs`,
`src/fit_zoned.rs`), which this release does not touch. The one path it would
have exercised that this release DOES touch — the full-resolution develop
funnel, where one guard line was added — was measured directly instead: a real
60 MP Sony ARW developed at full resolution, 9504 × 6336 out.
