# AutoShade user manual

The operating manual for AutoShade — the desktop app, the CLI, and the
embedded web UI; it describes the current release named in the README.
Downloading a release and the first run are in the README's
[Install and quickstart](../README.md#install-and-quickstart), and
[Install, upgrade, and uninstall on Windows](#install-upgrade-and-uninstall-on-windows)
below is what the installer does the second time it is run; what the program
is, what is new in it, and how it works are the README's opening sections;
subsystem boundaries are in [ARCHITECTURE.md](ARCHITECTURE.md) and the
algorithms in [TECH_STACK.md](TECH_STACK.md).

- [1. Open and inspect a photo](#1-open-and-inspect-a-photo)
- [2. Develop the image](#2-develop-the-image)
- [3. Add local masks](#3-add-local-masks)
- [4. Use versions and variants](#4-use-versions-and-variants)
- [5. Export](#5-export)
- [CLI reference](#cli-reference)
- [Lightroom and XMP interoperability](#lightroom-and-xmp-interoperability)
- [Configure and use the AI features](#configure-and-use-the-ai-features)
- [Privacy, trust, and paid-feature boundary](#privacy-trust-and-paid-feature-boundary)
- [Install, upgrade, and uninstall on Windows](#install-upgrade-and-uninstall-on-windows)

## 1. Open and inspect a photo

Use **Open photo…** (`Ctrl+O`), drag and drop, or **Open folder…**. The library
is read-only: AutoShade stores develop state separately and never rewrites the
source RAW. The viewer applies the photo's orientation before crop and mask
geometry, so every tool works in the displayed frame — and when a Lightroom
sidecar sits beside the RAW, the rotation Lightroom holds for that photo is the
one AutoShade opens it in, not the camera's original. A turn made here is
written back into the sidecar, so Lightroom picks it up too. The neutral view
is AutoShade's own conversion, not the camera JPEG; histogram and clipping
information are computed from the decoded image and also feed the AI verifier.
The develop starts from a **camera-matched base look**: a tone curve read off
the camera's own embedded preview, block by block, so the neutral develop
looks like the camera's JPEG rather than a flat conversion. Since v1.6.0 that
estimate is made on block means of the two pictures, and a photo saved by an
earlier version gets the current estimate the first time it is opened — here,
in batch export, in the web UI or with `apply` — and a toast says so; a photo
saved with no base look keeps none.

## 2. Develop the image

The Develop panel exposes white balance, exposure and tonal controls, RGB point
curves, HSL, color grading, texture, clarity, dehaze, noise reduction,
sharpening, vignette, crop, and lens-related settings, rendered through the
same engine as `autoshade apply`.

Every slider in the **Effects** fold renders since v1.5.0 too. The post-crop
vignette is centred on the **crop**, not on the frame — that is the whole point
of its name — with Midpoint placing the falloff, Feather softening it,
Roundness running from a rounded rectangle through the crop's own ellipse to a
circle, and the three Styles behaving the way Lightroom describes them:
Highlight Priority spares bright pixels channel by channel (and can shift their
colour), Colour Priority spares them by the pixel's brightness (and cannot),
Paint Overlay simply mixes toward black or white. Grain is sized in pixels of
the full-resolution photo like the Detail radii, and it is the same grain every
time you export — never a fresh sprinkle of random noise. These nine are
first-principles readings of operators Adobe has not published, so expect a
family resemblance to Lightroom rather than a pixel match.

Every slider in the **Detail** fold renders since v1.5.0: Sharpening with its
radius, detail and masking, and both noise reductions with their detail,
contrast and smoothness (before, eight of them only reached the Lightroom
sidecar). As in Lightroom, the radii are measured in pixels of the
full-resolution photo, so the canvas shows them the way the export looks once
it is shrunk to the canvas: a 1-pixel sharpening radius on a 61 MP frame is
subtle on screen and plain in the exported file. A slider whose Lightroom
default is not zero (radius 1.0, sharpen detail 25, the noise detail and
smoothness sliders 50) shows that default until you move it; setting it to 0
writes a real 0 to the sidecar. Since v1.6.0 the Sharpening amount is one of
them, with the default Lightroom itself uses for the kind of file — 40 on a
RAW, 0 on a JPEG or TIFF — so a RAW you never sharpened here is sharpened the
way Lightroom sharpens it by default, a sidecar that says nothing about the
amount lets Lightroom keep its own, and dragging the slider to 0 on a RAW is a
real 0. These operators are AutoShade's own, built from
what Adobe documents about each slider — close to Lightroom, not identical.

The **Lens** fold finishes the set since v1.5.0. **Remove chromatic
aberration** used to be carried to the sidecar and nothing else — it is an
instruction, not a number, so rendering it means running the solver it names:
AutoShade measures this frame's own red-to-green and blue-to-green
magnification error and adds the answer to the manual Red/Cyan and Blue/Yellow
sliders, in their own units, so a preview and the export agree and you can see
what it decided. **Defringe** renders too, with its purple and green Amounts
and the four hue sliders that set which hues each one acts on: where a
high-contrast edge carries one of those hues, the fringe loses its colour and
keeps its brightness, so a purple subject that is not on an edge is never
touched. Adobe has never published what its 0–100 hue numbers mean in degrees,
so that mapping is AutoShade's own — its defaults (30/70 purple, 40/60 green)
land on the violet-to-magenta and the narrow green a fast lens really fringes
with. A hue window on its own corrects nothing until its Amount is above 0.

The same fold gained Lightroom's two **profile correction strengths**
(Distortion amount and Vignetting amount, 0–200 with 100 meaning "exactly what
the profile says" and 0 switching that component off). They scale whichever
profile this photo has — and since v1.5.0 that can be an **Adobe `.lcp`** read
from the Camera Raw profiles installed on this machine, for bodies whose RAW
carries no correction data of its own. AutoShade never bundles or redistributes
Adobe's profiles; it reads the ones already on the computer, and a camera's own
in-RAW measurement always outranks a profile-database average. A body with
neither renders with no profile correction, exactly as before. If a Lightroom
sidecar says the lens correction was switched off, AutoShade switches it off
too — including the camera's own — so the canvas and Lightroom agree, and so
masks and the pixels beside them are in the same frame. The manual Distortion,
Vignetting and CA sliders are not the profile and are never touched by that
switch.

The Lens header's ● stays unlit for both profiles enabled from the camera's
calibration and profiles switched off by the imported Lightroom sidecar.
Changing a profile toggle from that open state or a manual correction
(including either profile strength) lights it.

The **Transform** fold used to show Lightroom's perspective values and say we
did not touch them. Since v1.5.0 it moves pixels. Vertical and Horizontal are
the two keystones — the shape a building takes when the camera is tilted up or
sideways — with Rotate, Scale, Aspect and the two Offsets beside them, and they
run as one step after the lens correction and before the straighten, so masks
and brushes stay on what they were painted on. Scale runs from 50 to 150, as
Lightroom's own slider does; a smaller or larger number in a hand-edited file
is read as the nearest end of that range.

Adobe has never published what a Transform slider does, so AutoShade measured
it: fifteen test photographs were edited in Lightroom, one slider at a time, and
each export compared against its own untouched version to work out the exact
move Lightroom had made. Scale, Rotate, Aspect and the two Offsets now match to
better than a percent. The two keystones match in shape, and differ in one way
worth knowing about — Lightroom also stretches the picture slightly along the
axis you are correcting, by an amount that depends on the LENS, and the test set
was not large enough to work out that rule. So a keystone in AutoShade frames a
little differently from the same keystone in Lightroom: a touch tighter on a
long lens, a touch looser on a wide one. The shape of the correction itself is
right.

**Upright** is the dropdown above them, and it is the honest half of that
sentence. Lightroom does not publish how its Upright solver works, but it DOES
write the answer into the sidecar — one matrix per mode — so on a photo you
corrected in Lightroom, AutoShade renders Lightroom's own result exactly, to
the last digit. The panel says so while it is doing it. On a photo Lightroom
never corrected, picking a mode here runs AutoShade's own solver instead: it
reads the picture's long straight edges, finds where they would meet, and
builds the turn that sends them parallel again — Level straightens, Vertical
stands the verticals up, Full does both, Auto is a gentler Full. A photo with
no strong lines gets no correction rather than a guess. Guided is the one mode
AutoShade cannot solve: it needs the guide lines you draw in Lightroom, and
those are not in the sidecar in any form, so only the matrix Lightroom already
wrote can be rendered.

A perspective correction can leave empty corners. Lightroom has a **Constrain
crop** switch for that, and AutoShade now obeys it: off (which is what every
Lightroom file in a real library says) the empty corners show and your own crop
is what removes them, exactly as in Lightroom; on, the crop shrinks about its
own centre — keeping its aspect ratio — until it sits inside the corrected
frame. An Upright correction leaves no corners to fill: Lightroom already
scales its own answer to cover the frame, and AutoShade's solver does the same.

Under those controls the fold now names the **profile** this photo is developed
through. Lightroom writes two: a camera profile (「Adobe Standard」 and the
like, a file Adobe installed on your machine) and, above it, a creative profile
— 「Adobe Color」 on almost every photo Lightroom has touched, or 「Adobe
Landscape」, 「Adobe Monochrome」 and the rest if you picked one. Since v1.5.0
AutoShade reads both and develops through them, which is a large part of why a
photo now opens looking like it did in Lightroom rather than flatter.

Both rows are read-only, and one of them carries a warning worth reading. A
creative profile has two halves: a baked tone curve with a few baked sliders,
which AutoShade renders, and a creative colour table, which Adobe stores in a
form nobody outside Adobe can read. When a profile has one, the fold says so
under its name rather than letting you assume the whole profile arrived. Colour
may therefore sit slightly beside Lightroom's on those photos; tone will not.

The **Curves** fold holds Lightroom's **parametric curve** too since v1.5.0:
Highlights, Lights, Darks and Shadows sliders (±100) under the point curve, then
the three splits that set where those regions meet. As in Lightroom the point
curve is applied on top of it, and both go to the sidecar. A split cannot pass
its neighbours, and moving a split changes nothing until a region slider moves.

The **Color Mixer (HSL)** fold holds the whole of Lightroom's Color Mixer since
v1.5.0. **Black & White** at the top of the fold develops the photo to grey and
swaps the eight colour bands for the **B&W mix**, where each band decides how
light the greys that came from that colour turn; colour grading still tints the
result, which is how a split-toned black and white is made. A photo whose
creative profile is a monochrome one — 「Adobe Monochrome」 and its relatives —
develops to grey with this switch still off, because in Lightroom the profile is
what made it black and white. Below the mixer,
**Point Color** adds one swatch per colour you want to move on its own: click
「💧 Pick a color」, then click that colour in the image, and the swatch appears
with its own Hue, Saturation, Luminance and **Range** sliders — Range widens or
narrows how much of the neighbouring colour it takes with it. A spot that is
almost grey makes no swatch, because no swatch could move it. Swatches you made
in Lightroom arrive with the photo and go back to the sidecar; a Lightroom file
with no swatches keeps its empty block exactly as Lightroom wrote it.

The **Calibration** fold is Lightroom's Calibration panel: the shadows tint and
each primary's hue and saturation. These seven are the FIRST thing applied to
the photo, so they move the colours every other section then works on — a
Lightroom photo usually arrives with values already in them, and they used to
be shown here without a way to change them.

The **HDR & SDR** fold sits beside Export, because that is what it governs. If
a photo was edited in Lightroom's HDR mode, its brightest stops sit ABOVE white
rather than clipped at it, and **Headroom (stops)** is how many. AutoShade only
ever writes SDR files, so what it renders is the same thing Lightroom would
publish: the SDR rendition, shaped by the seven sliders under it — Blend,
Brightness, Contrast, Highlights, Shadows, Whites, Clarity. **Blend** is how
much of the headroom reaches that rendition: −100 ignores it entirely, 0 is the
headroom the sidecar states, +100 is twice as many stops of it.

Those seven only render while **HDR edit mode** is ticked, exactly as in
Lightroom, where the panel is not there otherwise — so a value left over from
an HDR session you later abandoned cannot quietly re-tone the photo. The values
are still kept, still written back to the sidecar, and the fold still shows its
● so nothing a file holds is invisible.

No photo in the reference library ever turned HDR mode on, so this was the one
part of the develop chain with nothing to check against. A test photograph was
edited in Lightroom's HDR mode for that purpose, and the shoulder AutoShade
rolls the headroom off with is now measured against it rather than reasoned
about: it follows Lightroom's own curve closely at the top of the range, and
holds back a little in the mid-tones, where Lightroom darkens slightly more
than AutoShade does.

**🤖 AI Denoise now** in the Detail fold denoises the active card's pixels at
full resolution and lands the result as a new **◈ Denoised negative** card
carrying that card's develop; the card you started from keeps its pixels, and
there is no working-copy tier and no **Full-res** checkbox any more. A RAW is
denoised on its sensor mosaic, before demosaic: the sidecar measures the
frame's own noise model (variance = a·signal + b, per colour plane) and runs a
non-blind network on the packed colour triplets under a variance-stabilising
transform, which is what keeps the texture — against ground truth on a 61 MP
frame at its measured ISO-640 noise it scored 3.2–4.9 dB above the previous
SCUNet path, and 3.1–5.9 dB on the most detailed blocks
(`scripts/denoise_bench.py`). Since v1.5.0 the network is **AutoShade's own**:
the same architecture, trained for this pipeline on real noisy/clean pairs and
synthetic sensor noise instead of the general-purpose weights it started from.
The network now receives the honest measured noise level — measured across
the frame, not only by brightness. Noise is not the same everywhere in a
picture: on the wide-angle star frames measured it rose toward the corners,
and a night frame's dark foreground held less than a brightness-only model
predicts. Told one level for all of it, the denoiser
smoothed the middle of a star frame flat and left its sides half-cleaned. It
now measures how the noise varies over the picture and cleans every part by
its own amount. The old 0.78 sigma
scale left different amounts of grain at different ISOs; grain now comes back
explicitly from this frame's removed residual. The map into the model's range
still comes from the noise model, preserving the v1.4.1 highlight guards.
Isolated hot pixels stronger than 20 σ are mapped out of every RAW develop,
with or without AI denoise, before anything else reads the sensor data. A hot
pixel is a defect of the sensor rather than noise, so it no longer waits for
AI denoise to be switched on. The noise is judged where the pixel stands —
its own neighbourhood's, when that is brighter or rougher than the area
around it — a pixel in texture or on a blown highlight is left alone, and so
is one whose neighbours — all eight — show any light of their own: stars,
lights and highlights stay as they are. On the three long-exposure night
frames measured this changed 0.010 % of the picture or less; on six
ordinary daytime and dusk frames it mapped 0 to 6 sensor pixels, and a frame
without such pixels renders exactly as before. One limit is known: a point of pure-colour
light no wider than one sensor pixel on flat dark ground — single dots of a
distant red LED sign — cannot be told from a defect in one exposure and loses
that pixel. Only sensors with an ordinary 2×2 Bayer mosaic are mapped.
The fold's **AI denoise strength** starts at 71% for a RAW: higher is cleaner,
lower returns more of the frame's own luminance grain after demosaic, in
linear light. Every positive strength keeps the clean colour; exactly 0%
leaves the input untouched without running the model. 100% keeps the network's
complete output. The reference for the default is Lightroom Denoise
50, not a promise that different renderers produce identical pixels.
A baked PNG/TIFF/JPEG still uses SCUNet, its separate default is 50%, and its
colour-noise removal is complete from 50% up. The Export fold has its own dial;
both folds remember RAW and baked choices separately. On the first launch in
preferences era 2, old RAW choices reset once to the new default, with both old
percentages in the startup status; old shared values remain the baked choices,
and choices saved in era 2 survive later launches. While a ◈ card exists its
master is
the photo's negative: a later Reimagine sends it, and a Reverse-fit is solved
on it and lands on it (section 4).

Buttons follow one vocabulary: the gold button in a group is its main action
(Export on the toolbar, AI Analyze, Reverse-fit recipe, Apply in a brush
session, Save settings), icon-only buttons are squares as tall as the text
buttons beside them, and rows of equal actions in the side panels sit in
aligned columns. These rows share the prompt fields' 420 px readable ceiling
and stay left-aligned when the panel is wider; the panel itself can still
grow for the curve and HSL editors. A glyph in front of a label always means
the same thing —
🤖 an AI verb, ✓ finish, ✕ cancel, ＋ add, ↺ reset, 🗂 a folder, 🖌 paint, 💧
pick from the image.

**Save develop** (`Ctrl+S`) persists the recipe and, for a RAW, its XMP
projection in the per-user develop store (`%LOCALAPPDATA%\autoshade` on
Windows). Upgrading from Autoshop v1.1.0 or earlier moves the old
`%LOCALAPPDATA%\autoshop` folder there on first launch, in one step, and says
so; if a folder of the new name already exists nothing is moved or merged and
the app says that instead. A settings file still named `autoshop.local.json`
keeps being read until the next time settings are saved.
A neighboring Lightroom/ACR `.xmp` is
read only as the merge base; Save does not overwrite it. A baked image keeps an
AutoShade recipe but does not receive a RAW XMP, and neither does a card on
AI-generated pixels (✨ / ✎): its develop is saved, a projection left by an
earlier source develop is retired in the same save, and the status line says
so. To deliver the stored projection where Lightroom reads it, choose
**Export .xmp beside the photo**; replacing an existing neighboring sidecar
requires confirmation.

## 3. Add local masks

Open **Local Masks**, create a mask, then adjust the sliders inside that mask.
Shapes can be combined with Add, Subtract, or Intersect and can carry luminance
or color range restrictions. Linear, radial, brush and AI components export
that composition to Lightroom. Bitmap components remain a named export loss;
the component Invert control complements its shape before composition.

- **Linear gradient:** choose **＋ Linear**, then drag from the fully
  affected side toward the unaffected side; the shipped falloff eases softly at
  both handles. Hold `Shift` to lock an axis.
- **Radial gradient:** choose **＋ Radial**, drag the ellipse, then
  position, rotate, and feather it.
- **Brush:** choose **🖌 Brush** and paint. Use Erase to subtract, `[` and `]`
  to change brush size, and **Apply** to bake the stroke into a bitmap alpha.
- **🤖 Select subject:** runs local BiRefNet, with a named U²-Net fallback when
  the preferred backend cannot run.
- **🤖 Select sky:** runs local OneFormer ADE20K sky segmentation.
- **Point-prompted object:** imported object intent and ordered positive click
  gestures are re-derived locally with SAM 2.1.

AI mask rasters are cached with backend provenance, so a better backend forces
an honest re-derivation instead of presenting an older alpha as its result.

## 4. Use versions and variants

A variant is one card for the same photo: **▣ Original**, **✨ AI generated**,
**✎ Edited AI image**, **◭ Reverse-fit**, **◈ Denoised negative**, or
**▦ Stacked**. Each card combines its own base
pixels with one develop. `Ctrl+S` saves every card in the strip together.
Switching cards is navigation, not an edit; reopening returns to the card that
was active at the last save, not the last card viewed.

A **✨ AI generated** card is the generated image itself and never changes:
the first slider you move, paste, version load, AI analysis or in-place retouch
on it continues on a new **✎ Edited AI image** card right beside it, on the
same pixels, and the ✨ card stays pristine (a toast says so; hovering the ✨
label says it before you try). Undo, zoom and tools carry over — only the card
under the canvas changed. Editing an ✎ card edits that card. Both kinds save
their develop with `Ctrl+S` and receive no Lightroom XMP (no sidecar can
reproduce generated pixels). A photo saved by an earlier version with edits on
its ✨ card opens split into ✨ + ✎ once, as unsaved work, and `Ctrl+S` keeps it
that way.

### Stack several frames into one negative

Photographers shoot several frames of one scene for four reasons, and the
**Stack** section of the Develop panel merges them for all four. The card you
are standing on is the **reference** — the result keeps its framing and its
exposure — and the frames you pick join it. Pick the merge, then
**▦ Stack with other frames…**:

* **HDR merge** — an exposure bracket into one frame that holds the whole
  range. It measures each frame's exposure *from the pixels* rather than from
  metadata, divides it back out, and weights each sample by how trustworthy it
  is as a measurement, so a clipped highlight stops counting. The stops it
  recovers above the reference frame's white are handed to the **HDR & SDR**
  section, which arrives switched on with that much room — without it the
  recovered highlights would be in the file and nothing would show them.
* **Exposure fusion** — the same bracket with no HDR in between, blended
  band by band from wherever each frame looks best (detail, colour and
  exposure decide). A finished picture rather than data: no headroom can be
  read back out of it, and none is claimed.
* **Focus stack** — a focus sweep into one frame sharp throughout, each band
  taken from the frame that resolved it.
* **Noise stack** — repeated frames of a still scene averaged in linear light,
  with the readings that disagree withdrawn, so the signal adds, the noise does
  not, and somebody who walked through one frame is gone.

Handheld frames are aligned first: a global affine (shift, rotation, scale and
the breathing a zoom does between frames) refined by a local pass for a subject
that moved on its own. Tick **Shot on a tripod** to skip it: on frames that
really are registered there is nothing for the alignment to find, and looking
costs a little. Measured on a five-frame noise stack, the grain fell by 2.03×
with the alignment skipped and 1.97× with it running. The frames must all be
the same size; the merge runs at full resolution.

The result lands as a new **▦ Stacked** card carrying the develop you ran it
from, and the frame you started from keeps its own pixels. While a ▦ card
exists it *is* the negative: reverse-fit and reimagine read its master, and so
does a further stack. Its status line reports how far the aligner had to move
the worst frame, how much of the frame no source could cover, and — for an HDR
merge — the stops it recovered. A `.xmp` from it carries the sliders only; the
merged pixels live in the 16-bit master beside it in `./out`.

In the browser the same four merges are in the **Stack** panel beside Heal.
There is no variant strip there, so the merged master joins the session chain
the way a heal does: every later develop, export and download follows it, and
`Save` records it for reopening. On the command line it is `autoshade stack`.

A version is a numbered snapshot of one card's develop at one moment. **＋ Save
as version** writes `v<N>.recipe.json`, frozen `v<N>.mask-*.png` rasters, and
`.version-meta.json` provenance (`from_kind`/`from_id`, name, and `user` or
`auto` origin). Loading a version replaces the active card's canvas as one undo
step. `auto` versions are snapshots made by the backup gate before it replaces
a saved develop.

An AI-generated variant carries its look in pixels and has no editable XMP
develop. Reverse-fit estimates an engine recipe from that look — it reads the
✨ card's pixels, so select that card (an ✎ card's edits are your own sliders
over the same pixels, not a look to solve for); copy the fitted develop to
Original when you want an editable recipe and sidecar for the full-resolution
source. **＋ Save as version** snapshots an ✎ card's develop; a pristine ✨ card
has nothing to snapshot.

A **◭ Reverse-fit** card develops the photo's negative: the **◈ Denoised
negative** card's master while one exists (an AI denoise lands as that card
and never changes the ▣ card), else the ▣ Original card's own in-place master
after a heal or clone on it, else the loaded file. The fit is solved on that
negative, the ◭ card renders and exports from it, and the fit's save links it
in `pixels.json` so a reopen restores the same pixels.

A **◈ Denoised negative** card is the negative AI-denoised into its own
16-bit master. Develop it like the ▣ card — heal and clone stay in place on
it, a `.xmp` from it carries the sliders only (run Lightroom's own Denoise
there), and the export-time AI denoise sits out on it because its master is
already denoised.

**Spot removal you did in Lightroom comes across.** If a photo's `.xmp`
carries Lightroom's healing — dust, a power line, somebody in the background —
the Retouch panel opens with an **Imported removal** line saying how many areas
it found, and the canvas already has them off. Those repairs are AutoShade's
own: for all but the plainest kind Lightroom keeps its result in Adobe's store
rather than in the sidecar, so the panel also says how many areas Adobe
synthesised and leaves a **✨ Regenerate those areas** button, which paints
exactly those shapes into the shared brush mask and re-runs the generative
model over them (an empty prompt removes; the result lands as a new ✨ card
like any other fill). One thing to know when you save: a merge into an existing
`.xmp` keeps your original removal block untouched, but a sidecar written where
none existed carries no removals at all, and the save line names that.

A generative fill is not an in-place retouch. The model is shown the active
card's developed picture — its sliders and masks applied, in the uncropped
frame the brush paints in — the painted area is regenerated (an empty prompt
removes it), and the result lands as a new **✨ AI generated** card: its look
lives in its pixels, crop and straighten are not carried over, and the card you
filled from is unchanged, the ▣ negative included. The browser's Fill and the
CLI's `retouch` still composite onto the source's neutral develop.
**Adjust generated image** is GUI-only: the browser and CLI retain region
retouch and have no whole-image adjust entry.

Reverse-fit uses its own **Reverse-fit strength** dial in the Reverse-fit fold
(or `match --strength 0..1`) as its honesty budget; the Analysis fold's
Strength above it does not reach the fit. At or below the shipped 65% setting the historical path is
byte-identical, including white balance: a demand outside its budget remains
as-shot. Above 65%, WB demands outside the widened budget shrink along the
requested Kelvin/tint direction and are disclosed. The pre/post WB renders must
also pass the foreign-hue veto and a weighted rotation allowance, pinned at
0.05 through 65%, about 0.593 at 85%, and 1.0 at full strength. If no legal WB
remains, it is withheld and the recipe stays as-shot with a typed explanation.

Since v1.3.0, the full solve has a white balance too, not only the Atmosphere
path. It is solved from the same population estimator and then RENDERED and
checked against the target's own 12×8 cell means before it ships: if the
render did not move the frame toward the target, the recipe returns to as-shot
and says so. A same-frame pair whose light did not change therefore still
reports `temperature_k` unset.

**When a region was repainted rather than re-graded.** An AI variant that
recolours the sky and leaves its layout alone breaks the pixel-to-pixel
correspondence inside that region, and reverse-fit used to withhold the zone's
colour and tone controls on exactly that ground — correctly for the pixels,
wrongly for the region. The same 12×8 cell check now answers for a REGION too —
but only where that region's own structural reading says its pixels are not each
other's counterparts, because a region whose pixels do correspond may not
overrule them. Where it applies, the withheld move is rendered and put to the
target's own cell means over that zone, and it ships only if most of the
region's cells moved closer to their own targets *and* in the direction those
targets ask for. Per control class you will see one of: nothing at all (the
evidence was never in question); *withheld … zero-evidence hue bands*, the
sentence unchanged from before, for a region whose pixels do still correspond
and were therefore the ones asked; *shipped on REGION evidence* with the three
shares it was admitted on; *shipped at … of the solved move on REGION
evidence*, when the cells agreed on the direction of the move and refused only
its size — the largest share of the move they do vouch is what ships, and the
sentence prints the refused full move's verdict beside the shipped share's
three shares; or, for a region past the pairing line whose cells
said no, that same refusal with the shares it was decided on — including the
case where the cells *abstained* because nothing in that region carried
measurable evidence. Where the cells were asked, a refusal is a measurement now
rather than a silence. A zone whose own structural reading
is past the pairing line also says which estimator solved its tone, because a
per-pixel regression reads a repainted texture's contrast low.

**A step the target has is not a seam.** Since R37 the boundary gate that
holds every zone, band and tile reads the target too: where the target's own
boundary steps — a sharp horizon under a hazy source — the correction may
reproduce that step, and only what it introduces beyond it is charged against
the seam ceiling. The target's step is read as an average over each evidence
cell, so a repainted texture's pixel noise neither grants nor refuses
anything. The pass line says how much the target asked for (`of which the
target's own boundary asks …`).

**A correction that shrank is judged again.** The boundary gate negotiates an
over-budget correction down to a strength `k` rather than refusing it, and since
R39 (v1.6.0) the correction that ships at that strength is held to one more
test: it must leave its own zone no worse than the same render without it, and
the frame within the drift its attachment was allowed. A correction admitted at
full strength can fail this once shrunk and is then refused with its readings
(`refused after its boundary shrink to k=…`); the survivors are gated again
from full strength.

**A tile edge cannot hide a step behind a smooth sky.** The reference pair's
sky tile that shipped at k = 0.134 — a pale block no gate had judged — passes
the test above (what its acceptance read improves), and what let it through
was the boundary ruler itself: it budgeted each crossing by the scene's own
change over three pixels, so a smooth gradient bought a tile edge a step of
its own size. Since R40 (v1.6.0) a hard-edged mask's crossings are read as
discontinuities — the step less the sky's own trend on either side — and
budgeted only by a discontinuity the scene already has there, so in smooth
sky the budget is one code value at the crossing and the tile is shrunk to
fit it or refused. On the reference pair the re-fit under this ruler no
longer attaches that tile at 0.85; at 1.0 it attaches at k = 0.119 (0.161
before), and what remains of its edge is a soft ramp of about two code
values across ten pixels of a 1000 px render, not a step.

**A sky can earn bands.** When the sky or land residual has a measured vertical
colour pattern, the fit trials two or three overlapping corrections in place
of the single zone. Each band must pass the same evidence and quality gates,
and the set must improve the zone without worsening the boundary readings
at any band break or at the horizon. Those readings compare the render with
the target as cell averages (since R37; ranking single pixels against a
repainted texture had refused every band on the reference pair): no cell may
move away from the target by more than a seam's worth, and the average may
not move by more than a code. The fit also checks the target's actual step
within each boundary cell; shrinking a replacement preserves the original
correction's tone while reducing the new band differences.
The mask list names each `sky · band 2/3` with its Intersect components visible.
An unstructured residual keeps the single correction and says why. Hard spatial
tiles now use four intersecting gradients; a guided edge retains its bitmap
only when the native trial fails the shared gates or fits the photo worse
than the bitmap on the tile's own cell or on the whole frame — the tile's
note prints both residuals. The native trial's gate reads the tile's edge
where the preview paints it under the photo's lens profile (R38): before, a
contour taken at the tile's stored coordinates sat a few pixels off that
edge, the seam ruler measured nothing there, and a gradient tile could ship
as a visible rectangle in the sky. The save
line therefore counts only the Bitmap corrections/components that remain.
Lightroom 9.4 reads the native composition and keeps it through its own
rewrite of the sidecar (measured 2026-09-12 on the reference pair's sidecars:
every correction, gradient and Select Sky component came back; only
AutoShade's intent attributes did not, and since v1.3.1 the zone roles ride
in the payload and in the corrections' names instead). AI alpha and local
recolour gains still have their existing separate disclosures.

**The colour field.** From the 65% default up (never below it), the fit may
also attach a smooth 12×8×8 local colour/tone field — the residual its masks
and range bands cannot shape; at the default it is held to a per-channel gain
of 0.35, at 85% 0.61, at 100% 0.80, so the default's field is the most
conservative of the three. It appears in the develop panel's **Local Masks**
section as its own row, `▦ Colour field · engine-only`, with an eye to mute it
and an Amount slider; deleting the row removes it. It is the first control in
this app with **no Lightroom equivalent at all**: classic XMP has no
coordinate system for a smooth local field, so the save line names it among
the things Lightroom cannot render; since v1.3.1 the `.xmp` beside your RAW
still carries it inside AutoShade's own payload, and reopening that sidecar
restores it. Copy/paste to another photo drops it and tells
you — its cells are measured on this frame's own geometry and mean nothing on
someone else's picture. At or below 65% no field is attached and the recipe
file does not carry the key at all.

Where a region was repainted, the field is solved twice and the target decides
which answer each of its 96 cells keeps: the ordinary structure-weighted solve,
or a support-free one with a wider per-channel gain that the Reverse-fit
strength sets. A cell takes the second only if rendering it moved that cell toward its
own target; the rationale line says how many of the measured cells did. A cell
the pixel evidence could not read at all — a featureless sky the target
re-synthesised smooth reads as "texture gone" to the structural instrument — is
read on the region pairing instead when it lies inside a region the segmenter
found in both frames, and takes the second solve on the same cell-mean verdict
as the measured cells; a second rationale line counts those.
The field is then dropped whole if it makes the frame worse, or if it improved
the frame by making the sky or the land zone worse.

With **Zoned fit (sky)** enabled, reverse-fit always solves the global recipe
first. Successful segmentation adds up to four disjoint semantic class bitmap
corrections; each region selects Full or Atmosphere independently. If
segmentation is disabled or unavailable, the same entry automatically tries
evidence-gated native luminance ranges and then colour ranges instead, and if
no band is accepted the global recipe is kept. A range band is retained only when its composed
evidence-weighted frame is no worse than the running global/banded result.
The historical two-region route is the default. Enable **Up to four semantic
regions** in the GUI, or pass `--regions 4` on the CLI, to opt in to the
expanded route. It performs one OneFormer inference per frame and may take
longer. The default two-region path keeps its single corrections unless
residual-earned bands pass every gate. A zone whose dials did not move gets
one typed `ZONE_ALREADY_MATCHED` note.
Generated range masks persist as editable **Luminance range** cards with their
four ordered bounds and **Colour range** cards keyed to one hue band's mean
colour; their sentinel-hosted range components project to Lightroom XMP as the
masks Lightroom itself writes. The sky and land zones project too, as
Lightroom's own **Select Sky** mask — the land zone is that same component
inverted — so Lightroom rebuilds its own sky alpha from them, while the raster
AutoShade renders from is its own (the save line says 「AI masks ×N re-derived
locally — not Adobe's raster」). **Invert** now reaches the sidecar on a brush
or AI mask as well as on a gradient, and a Lightroom mask that arrives inverted
renders inverted here: both used to drop the flag silently, so an imported
inverted sky selection painted the sky it was meant to exclude. The opt-in
four-class region bitmaps, retained refined tiles and free-form field masks
remain engine-only with the named bitmap loss. Native gradient tiles and
semantic bands carry their complete geometry composition.
The luminance family runs first and the colour family on the frame it leaves,
each gated as a stage of its own.

The mechanics — population-scoped verdicts, the local-field ceiling, quadtree
tiles, and guided refinement — are in [What is new here](../README.md#what-is-new-here)
§5–§8. What you see in use: both analysis rasters share one geometry (the
target is resampled into the source's analysis thumbnail), so a one-row
rounding difference can no longer switch the structural evidence gate off; the
global recipe and frame-wide range bands are judged on the whole frame, a
semantic zone or spatial tile on its own members. The analyzer produces
numbers only and never enters the recipe, the engine, or the sidecar; after
every producer the rationale states its ceiling, whether the remainder is
band-shaped, tile-shaped, linear or free-form, which stage it skipped when a
producer already reached the ceiling, why a field-proposed band was absorbed
or refused, and which luminance bins vary too much in space for a value band
to describe them (bins 3 and 4 on the calibration pair, at 29.1/255 and
28.7/255 against a 15/255 line). Shape is read only on the pixels the field
actually measured, so an unmeasured region cannot pose as structure; a
remainder the 4x4 tile means do not explain halves the quadtree's budget from
four tiles to two, and the quadtree stops at a 4x4 grid and that cap.
Range masks are never spatially refined, neither luminance nor colour: the
guided filter refines silhouettes and tile collars, and an observed domain is
not something it has evidence about.

The free-form field-mask pass then consumes only the remainder not already
covered by accepted tiles. It uses the field's frozen per-pixel weight, keeps
opposite signs in separate 4-connected components, and discloses every
proposal and typed refusal before or after fitting; the layer is enabled with
the field by default and is disabled whenever the field layer is disabled; there
is no separate user-facing switch.

## 5. Export

Open Export with the toolbar, `Ctrl+Shift+E`, or `Ctrl+E`. Choose JPEG, 8- or
16-bit PNG, or 8- or 16-bit TIFF; set JPEG quality, long-edge size, output
sharpening, and sRGB, Display P3, or Adobe RGB delivery color space. Resizing is
the last step, uses Lanczos3, preserves aspect ratio, and never enlarges a
smaller image.

A header's ● describes this photo. Saved preferences — Export's delivery
settings and the AI Style, Strength, and Reverse-fit strength dials — never
light it. The AI header lights only for a verdict on screen or a typed Direction.
The Export header carries no ●; hover over the toolbar's Export button to read
the current delivery summary.

**🤖 AI Denoise on export** runs the AI denoise inside every full-resolution
delivery — a RAW on its sensor mosaic, a baked source through SCUNet — at the
Export fold's own **Export denoise strength** dial: the same law as the Detail
fold's dial and the same 71% RAW / 50% baked start, but its own setting: moving one never
moves the other. The batch render skips it, and so does a ◈ Denoised card,
whose master is already denoised. The export summary echoes the amount ("AI
Denoise 71%") and carries none on a ◈ card.

CLI exports use q95 sRGB. `--long-edge N` is available on `apply`, `auto`, and
`batch --render`; `0` or omission means full resolution. It is deliberately an
export option rather than a recipe field, so one recipe can deliver both a
master and a web copy.

## CLI reference

The following commands and flags match the v1.0.0 command definitions in
`src/main.rs`:

```text
autoshade decode <src> [-o|--out FILE]
autoshade analyze <src> [-o|--out FILE] [--guidance TEXT] [--style 0..1] [--strength 0..1] [--adherence 0..1] [--embed|--no-embed] [--deep] [--reference-image]
autoshade apply <src> <recipe.json> (-o|--out) FILE [--long-edge N]
autoshade auto <src> [-o|--out FILE] [--guidance TEXT] [--style 0..1] [--strength 0..1] [--adherence 0..1] [--embed|--no-embed] [--deep] [--reference-image] [--denoise] [--denoise-strength 0..1] [--denoise-model NAME] [--long-edge N]
autoshade denoise <src> [-o|--out FILE] [--strength 0..1] [--model NAME]
autoshade batch <dir> [--render] [--limit N] [--include-baked] [--jobs N] [--long-edge N]
autoshade eval <dir> [--xmp-dir DIR] [--limit N] [--jobs N] [--fresh] [--state FILE]
autoshade style-index <dir> [--xmp-dir DIR] [--embed|--no-embed] [--describe]
autoshade style-index --looks <dir> [--embed|--no-embed] [--describe]
autoshade style-query <photo> [--direction TEXT] [--style 0..1] [--adherence 0..1] [--embed] [--distil]
autoshade reimagine <src> --prompt TEXT [--fidelity high|low] [--quality low|medium|high|auto] [--fidelity-retry] [-o|--out FILE]
autoshade match <src> <target> [--render] [--zoned] [--regions 2..4] [--strength 0..1] [--style-prompt] [--ai-judge] [--deep] [--negative RAW] [-o|--out FILE]
autoshade correspond <source> <target> [-o|--out FILE]
autoshade retouch <src> --mask FILE [--prompt TEXT] [--quality low|medium|high|auto] [--full-res] [-o|--out FILE]
autoshade heal <src> [--mask FILE] [--no-auto] [--full-res] [-o|--out FILE]
autoshade stack <frame1> <frame2> [frame3 ...] [--kind hdr|fuse|focus|noise] [--no-align] [--long-edge N] [-o|--out FILE]
autoshade serve <dir> [-p|--port N]
autoshade recipe-schema
```

`<src>` is a RAW or baked image. For commands that save develop state, baked
sources get recipe JSON but no RAW XMP. `auto` is `analyze` plus render.
`batch` analyzes RAWs by default, skips baked photos unless `--include-baked`
is set (avoiding duplicate analysis and billing for RAW+JPEG pairs), and
defaults to three photos in flight; `--long-edge` on `batch` requires
`--render`. `eval` defaults to serial work and resumes from its state file.
Denoise-strength/model overrides require `--denoise` on `auto`. A denoise
strength defaults to 0.71 on a RAW and 0.5 on a baked source, on every surface
(`denoise` and `auto --denoise`, the web export, both GUI dials). On a RAW the
network cleans the sensor mosaic at honest sigma; the dial returns only
linear-light luminance residual after demosaic and calibration; `denoise`
then writes a neutral 16-bit develop of it, and `--model` (a SCUNet tier) does not apply. On a baked source
SCUNet runs on the pixels: the value blends the luminance, colour noise is
removed in full from 0.5 up, and 1.0 is the model's whole output. `retouch`
without `--prompt` removes what the mask covers — the area is continued from
its surroundings and nothing new is put there; the GUI's and the browser's
Generative Fill treat an empty prompt the same way.
`stack` takes two or more frames of one scene, the FIRST being the reference
whose framing and exposure the result keeps, and writes a 16-bit master; it
prints each frame's measured exposure and alignment travel, and `--no-align`
skips the alignment for frames shot on a tripod. After an HDR merge it also
writes a recipe beside the master with the SDR rendition switched on and the
recovered stops filled in, because that is the only way to reach them.

**`match` on a denoised or stacked master (v1.6.2).** `match` fits what the
desktop app fits. A RAW is fitted
on a neutral develop of its sensor frame with the photo's calibration —
camera base look, lens profile, as-shot white balance — composed into the
solve. A master `denoise` or `stack` wrote is a baked image that carries no
calibration of its own, so name the RAW it was made from (for a stack, the
first frame) with `--negative`: the calibration then composes on top of the
master's pixels, exactly as the desktop app fits a ◈ Denoised or ▦ Stacked
card, the fit reads the same structure the RAW's fit reads, and `--render`
shares the RAW's frame. Without the flag a baked source is fitted as it
stands, with no calibration; on the reference pair that read the denoised
master's sky at a structural divergence of 0.716 against the RAW's 0.649 —
across the 0.65 line into the bounded atmosphere solver — and rendered
without the lens profile. A RAW source refuses the flag: it is its own
negative, and `match` on it describes the RAW itself, which is what its
saved develop and its Lightroom sidecar render afterwards.

### Where your `.xmp` sidecars are — `--xmp-dir`

`style-index` and `eval` pair each RAW with the `.xmp` sidecar your editor
wrote. By default that is the file **beside the RAW**, which is where Lightroom
and ACR put it. If yours live somewhere else — an exported catalogue, or a
photo volume you cannot write to — point `--xmp-dir` at that folder and three
places are searched, in order:

1. `<xmp-dir>/<the RAW's folder relative to `<dir>`>/<name>.xmp` — a sidecar
   tree that **mirrors** your library;
2. `<xmp-dir>/<name>.xmp` — one **flat** folder of sidecars for a nested
   library;
3. `<the RAW's own folder>/<name>.xmp` — beside the RAW, as before.

The extension matches in any case (`.xmp` or `.XMP`) on every platform,
Windows and macOS alike. The stem does not: a sidecar belongs to the photograph
whose name it carries exactly.

`style-index` now also says which RAWs it **skipped** for want of a sidecar —
the count, and the first ten by name. A library that indexed 40 of 2,000
photographs used to look exactly like a library of 40.

### Rebuilds only measure what changed

A `style-index` build caches what it measured per photograph — the 14 camera
features, the SigLIP image vector, the vocabulary scores, the description and
its text vector — in `style-exemplars.json` beside the index, keyed by the
content of the frame it measured (the same key the description cache has always
used). A rebuild:

* **reuses** a photograph whose file is unchanged and whose cached answers cover
  the passes you asked for — no decode, no model call at all;
* **recomputes** anything new, edited, moved or rotated, and anything whose
  cached answers came from a different checkpoint, phrase list or prompt;
* **retires** entries for photographs that have left the library;
* still re-reads every `.xmp`, because your sliders, curve, colour families and
  mask habit can change without the pixels changing;
* still recomputes the index's normalisation over the **whole merged set** —
  adding one photograph legitimately moves the mean and deviation of every
  normalised dimension, so those are never cached.

Each build prints one line saying so:
`style index cache: reused N, recomputed M, removed K, skipped-for-sidecar S`.
A rebuild where every photograph is reused starts no model sidecar at all —
neither the 1.5 GB SigLIP checkpoint nor the 4.3 GB Qwen one is loaded. The
cache is only ever a saving: deleting `style-exemplars.json` costs time, never
correctness, and a corrupt or foreign one is rebuilt with a printed reason.

**Built your index on v1.2.0 or v1.2.1?** If your library carries any HSL or
colour-grade edit, that index was written correctly but refused on load
(`exemplar 0 has an unsupported setting key`), and the Style control read
nothing. Run `style-index` once on v1.2.2 or any later release; the build is the
same, only the read was wrong.

`style-index --looks` builds the separate finished-photo look library; it never
adds camera features or develop settings to those records, and it is capped at
**500** finished photos (a curated set of reference grades, not an archive —
the RAW half's 5,000-exemplar cap and this one share a 228 MiB index envelope).
`style-query` is an offline diagnostic that prints the weights in force, the
exact retrieval terms behind every ranked neighbour and look — each weighted
term beside the raw cosine it came from — each neighbour's local-work counts
(`masks=… sky=… subject=…`), the VOICE the reference block will be spoken in
(`Ceiling` / `Target` / `Background` — pass `--adherence` to forecast a
different dial than the shipped 0.65), and the proposer reference
blocks, including the explicit reason a look library is unreachable when no
embedding vector is available — and with `--distil`, the distillation a develop
would apply: every channel including the ones it refuses, the Style pull, and
how many neighbours are black-and-white and therefore take no part in the
mixer. It prints; it never renders.

A RAW index build also reads the **masks** in each sidecar, and summarises them
as a habit: how many you enabled, how many carry a Range Mask, and per use —
sky, subject, foreground, range, other — a count and the average strength of
ten local sliders (in-mask temperature and tint included), plus whether the
mask carries its own local curve. It reaches the proposer as one sentence ("3 of 4 mask the
sky (linear from the top: exposure -0.6 EV, highlights -25) …"), so the AI
places its own masks the way you place yours. **No mask shape is copied or
averaged**: geometry belongs to one frame, and only counts and slider averages
cross between photographs. The Range Mask count is taken from both the imported
recipe and the import's own refusal notes, because a Range Mask in an encoding
this engine does not model is dropped on the way in — counting only what
survived would report "none use range masks" about a library that plainly does. The build prints what it learned and, separately,
any mask content it could not read whole (an unresolvable AI mask, a brush
table it refuses) so the summary is never mistaken for a complete one. An index
built before this feature keeps working; its reference block simply says
nothing about local work, and nothing needs rebuilding to keep using it.

`--embed` opts into the local SigLIP 2 sidecar for that run and `--no-embed`
refuses it; either flag wins over the environment, and neither writes to it.

`--describe` adds the local **look-description** pass to an index build. A
second local model (Qwen3-VL-2B-Instruct, Apache-2.0) writes ONE short sentence
per photo about its *grade* — white balance lean, tonality, contrast,
saturation and colour treatment, finishing, mood — and never about the subject.
That sentence is what the SigLIP text tower embeds for that record, in place of
the fixed attribute tags, and it is what `style-query` prints beside the
`desc=` term and what the proposer's reference blocks carry after the tags.

The pass needs `--embed` (the prose only reaches the ranking through the text
tower), and the **first run downloads about 4.3 GB** of weights into
`python/weights/` — every file pinned to a 40-hex Hugging Face commit and gated
on its own sha256 and exact byte count. Since 2026-09-20 the download asks
AutoShade's own mirror of that exact revision first and the original host
second, so a model whose upstream repository is renamed or removed still
installs; the checksum decides either way, and a mirror that disagreed with it
would be refused exactly as a bad upstream download is. It is off by default on
both front ends. Nothing leaves this machine and nothing is billed. Descriptions are
cached by frame CONTENT in `style-descriptions.json` beside the index, so a
rebuild only describes the photographs that actually changed; editing the
prompt bumps a version that invalidates the cache rather than serving the old
prompt's answers for ever. In the desktop app the same switch is the *Describe
looks with the local vision model* checkbox, which stays greyed out until the
embedding checkbox above it is on.
`--adherence 0..1` picks the prompt tier the proposer and verifier are told:
`<=0.40` Hint, `0.40..0.70` Direct, above `0.70` Brief, default `0.65`
(Direct). It never moves a render bound. It chooses the prompt tier and, since
v1.2.3, who leads: at Direct or Brief the direction leads and the library's
style pull is not applied (see *Who leads* below); at Hint the library leads.
It does nothing without a `--guidance` direction, which is why the desktop app
greys the slider out until Direction has text.

Every setting below is named `AUTOSHADE_*`. Up to v1.1.0 the app was called
Autoshop and these variables were named `AUTOSHOP_*`; the old spelling still
works everywhere, warns once naming its replacement, and is removed in the
release after this one. Where both are set, the `AUTOSHADE_*` one wins.

Six environment overrides steer retrieval, each read in exactly one place:

| Variable | Effect |
|---|---|
| `AUTOSHADE_STYLE_EMBED` | `1`/`0` — use the SigLIP sidecar. Set (any value) beats the GUI preference; `--embed`/`--no-embed` beats both. |
| `AUTOSHADE_STYLE_DESCRIBE` | `1`/`0` — run the local look-description pass during an index build. Set (any value) beats the GUI preference; `--describe` beats both. It never turns the embedding on by itself. |
| `AUTOSHADE_STYLE_EMBED_WEIGHT` | `W_EMB`, the query-image ↔ exemplar-image cosine block. `0` reproduces the 14-dimension ranking exactly. |
| `AUTOSHADE_STYLE_TEXT_WEIGHT` | `W_TXT`, the Direction-text ↔ exemplar-image term, scored after each exemplar's text hubness is subtracted. Ships at `0.5`: it spent one batch at `4`, where the corrected re-measurement showed the ranking collapsing onto a few hub exemplars; it shipped at `0` while the only query text available to the harness was a tag string. Re-tested in v1.2.4 against typed short Directions: 0.5 costs nothing measurable on the settings objective and is the largest weight that leaves the corpus open. |
| `AUTOSHADE_STYLE_DESC_WEIGHT` | `W_DESC`, the Direction-text ↔ exemplar-description term. Ships at `0.5`. It shipped at `4` when both sides of the term were tag strings; with real prose that point measures *worse* than switching the term off, so it was re-fitted. |
| `AUTOSHADE_STYLE_LOOK_WEIGHT` | `W_LOOK`, the look-library image term. |
| `AUTOSHADE_SEND_REFERENCE_IMAGE` | `1`/`0` — also send the retrieved reference photo itself (not just its text) with `analyze`/`auto` proposals; `--reference-image` turns it on per run. Destination-trust: only your own environment or user-level settings can set it — a downloaded photo pack's `.env` cannot, because it decides whether your photograph goes on the wire. `batch` never sends one. |

All four weights parse the same way: trimmed, and taken only if finite and
non-negative — anything else falls back to the shipped default, because a
negative weight would rank the *least* similar photo first.

`match` itself is local inverse rendering and needs no key. Its optional
`--ai-judge` and `--deep` review paths do; `--deep` permits one guided retry.
`heal` can use a supplied mask offline, while its automatic detector uses the
vision role.

## Lightroom and XMP interoperability

AutoShade reads and writes sidecar XMP for global settings, point curves, the
parametric curve, HSL, the B&W mixer, Point Color, camera calibration,
crop, and supported local corrections; the writer merges owned fields into the
existing document and preserves unmodeled content byte-for-byte instead of
round-tripping the whole file through a general XML serializer. Linear and
radial masks round-trip as editable geometry. Lightroom brush dab streams are
imported from the sibling `MaskBrushTable`, validated and Brotli decoded, then
rendered with AutoShade's measured brush model. Classic XMP does not contain
Lightroom's computed subject/sky/object alpha or arbitrary bitmap alpha, so
AutoShade preserves the selection intent and clearly re-derives the mask with
its own local model; generated image variants remain generated pixels until
reverse-fit produces an editable recipe.

**The sidecar carries the whole develop (v1.3.1).** Everything above is
what the Camera Raw settings can say. A sidecar AutoShade writes also carries
the develop itself — the recipe exactly as the app holds it, plus the mask
rasters nothing can re-derive (the fit's bitmap tiles and zone alphas) — as
properties in AutoShade's own XMP namespace on the same document. Lightroom
9.4 preserves those byte for byte when it rewrites the file (measured
2026-09-12: a 15 KB recipe and 250 KB of rasters came back unchanged, moved
into XMP's compact attribute form), while it drops unknown `crs:` items and
AutoShade's per-mask intent attributes. When such a sidecar is opened again,
AutoShade reads the Camera Raw settings as before and then reconciles them
with the payload: where Lightroom changed a value, Lightroom's value wins;
everywhere else — the colour field, a muted mask, a bitmap tile, a zone's
role, an exact slider position, the calibration anchor — the payload's exact
value is restored. Rasters are placed beside the develop on a restore (a
different file already under the name is left alone and the sidecar's copy
takes a `-2` name, which the status line says). A payload this build cannot
read is disclosed and the Camera Raw settings stand alone. The payload adds
roughly 15 KB for a full reverse-fit recipe and 80 KB per zone alpha; rasters
past a 6 MiB budget are left out and named in the save line. Two smaller
changes ride with it: `ColorNoiseReduction` is now always written, so a photo
AutoShade shows without colour noise reduction no longer gets Lightroom's RAW
default of 25 on top; and a sky/land zone whose intent Lightroom stripped is
still recognised from the name the writer gives it (`sky`, `land`,
`sky · band 2/3`).

## Configure and use the AI features

Open **Settings** (the ⚙ button at the right end of the toolbar) to configure the image/vision role and the analysis-verifier
role. The image role uses an OpenAI-compatible API for visual proposals and
generative images. The verifier defaults to the signed-in `claude` CLI over
OAuth, receives statistics and recipe data rather than image pixels, and can
instead use an API provider.

The same roles can be configured from the environment: `OPENAI_API_KEY` serves
the image/vision and generative role; `AUTOSHADE_ANALYSIS_API_KEY` is used only
when the verifier is set to API mode. Settings are saved in the per-user
`autoshade.local.json`; do not put real credentials in the repository. A
`./autoshade.local.json` in the current working directory may select
model/provider preferences but cannot supply API credentials, endpoints,
executable/script paths, or output destinations, so an opened photo folder
cannot become a credential or path override.

**Python interpreter.** The AI sidecars run under a Python 3 that AutoShade
does not bundle. Settings carries a **Python interpreter** field with a
**Detect** button beside it; Detect looks in the standard install locations and
fills the field with the first one that actually RUNS — it executes
`--version` rather than trusting that a file exists, because a Mac without
developer tools has a `/usr/bin/python3` whose only behaviour is to offer to
install them. The macOS candidates, in order, are `/opt/homebrew/bin/python3`
(Apple-silicon Homebrew), `/usr/local/bin/python3` (Intel Homebrew), the
python.org framework at
`/Library/Frameworks/Python.framework/Versions/Current/bin/python3`, and
`/usr/bin/python3` last; you can also type a full path instead of pressing
Detect. Finding none is reported as such rather than leaving the field
silently unchanged. Blank means the platform default: `python` on Windows,
`python3` elsewhere.

The field matters most on macOS, where an app launched from Finder inherits no
shell environment and the variable below therefore cannot be set for it at all.

| Variable | Effect |
|---|---|
| `AUTOSHADE_PYTHON` | The interpreter the sidecars are launched with. The same setting as the Settings field; the environment wins where both are set. |
| `AUTOSHADE_WEIGHTS_DIR` | Where all six sidecars keep downloaded model weights. Defaults to `weights/` beside the scripts — except inside a macOS `.app`, where it defaults into the develop store, because the bundle is signed and read-only. |

Both are *destination* settings — they name a program to execute and a
directory to write into — so neither may come from a `./autoshade.local.json`
sitting in the working directory. Only the environment or the per-user
settings file can supply them.

- **Analyze:** choose **Analyze** in the AI panel or run `autoshade analyze`.
  The vision advisor proposes bounded sliders and masks, a data-only verifier
  checks the proposal, and normal visual review may attempt one revision;
  `--deep` permits additional bounded rounds. Accepted output remains a normal
  recipe and XMP.
- **Style match/read:** build the RAW+XMP style reference library with the GUI
  (**AI › Reference libraries › My Lightroom edits library**) or `style-index`,
  and optionally build a separate finished-photo look library with
  `style-index --looks` (**AI › Reference libraries › Finished-photo look
  library**). Both libraries are read only while the Style control is above 0,
  so at Style 0 the two switches that only feed an analysis — the reference
  photo and **Use look library** — are disabled. Nothing else is: the folder
  pickers, both Build buttons and the two retrieval-engine switches stay usable
  at any Style value, because they decide what a *build* computes as well as
  what a query is matched on, and you build a library before raising Style onto
  it. The look library is retrieved through the SigLIP 2 embedding alone, so
  its **Use look library** switch stays disabled until **Use SigLIP 2 look
  embedding**, one rung above it, is on. The Style control retrieves similar prior edits
  and pulls the proposal toward them with `style_pull` (0.18 at the shipped
  Style 0.3, full at Style 1.0 — at 1.0 a control that has a target ends ON it).
  It pulls the twelve global sliders, the 8-band mixer's saturation and
  luminance, the colour-grade wheels, the master tone curve's shape and each
  mask's slider amounts — never a mask's position or size. A control is pulled
  only where your past edits AGREE on a direction; where they cancel out, or
  where you never touched that control, the AI's own choice for this photograph
  is kept. The rationale names every field that moved. Look records guide the
  proposer only. The embedding switch
  is opt-in and reports how many indexed records carry vectors. Strength
  independently controls the fit budget and confidence cap; Direction adherence
  chooses Hint, Direct, or Brief wording when a Direction is present.
- **Who leads, Direction or your library (v1.2.3):** write a **Direction** and
  leave **Adherence** at its default 65 % (or higher) and the direction leads.
  Your library is still retrieved, still ranked by that direction, still shown to
  the model and still named in the rationale — but it arrives as BACKGROUND for
  continuity ("where these habits and the direction conflict, follow the
  direction"), and the `style_pull` above is **not applied at all**, whatever the
  Style slider says. That is the whole point: measured on one frame at Style 100 %
  and Strength 90 %, three directions as far apart as *dark moody low-key,
  teal-and-orange*, *warm golden tones with lifted matte shadows* and *vivid
  saturated colours, punchy high contrast* all came back inside the library's own
  cool hazy register. Drop Adherence to 40 % or below (tier **Hint**) and the
  library leads again, exactly as it did in v1.2.2 — with no Direction at all, or
  a blank one, nothing about the Style control changes. A develop that skipped the
  pull says so in its rationale and names the tier that decided it. The judge that
  reviews the finished frame is briefed the same way: with the direction leading it
  is told your past-edit look is CONTINUITY, not the brief, so a revision it buys
  cannot be spent walking the direction back toward your library.

  All three surfaces can choose. CLI: `--adherence` on `analyze` and `auto`.
  Desktop: the **Adherence** slider, active once Direction has text. Browser
  (`autoshade serve`): the **Adherence** slider beside **Style influence**, and
  for anything driving that HTTP API directly, the `POST /api/analyze` body takes
  an optional `adherence` field (0..1). Omitting it means 0.65 — the same default
  every other surface has — so a client written before v1.2.3 sends the same
  request it always sent.
- **Reimagine:** enter a prompt in the AI panel or use `reimagine` to create a
  generated, lower-resolution target. `--fidelity high` (the default, and the
  GUI's mode) tells the model to re-develop the same photograph, not repaint
  it. The structural divergence **D** against the sent input is disclosed;
  `D ≥ 0.35` warns that a reverse-fit of that result will fall back to
  Atmosphere mode, and the opt-in `--fidelity-retry` (a GUI checkbox as well —
  off by default, it buys a second image) regenerates once and keeps the
  closer result. Use **Reverse-fit** or `match` to infer a deterministic recipe
  and apply it to the original RAW at full resolution.

- **Adjust generated image · paid API:** the fold directly after Reimagine in
  the AI panel edits the selected **✨ AI generated** card or its **✎** edit.
  It has its own prompt and remembered high/medium/low quality. With no brush
  strokes, enter what to change (for example, "make the sky bluer") and click
  **✨ Adjust** to edit the whole image. With strokes in the shared brush mask,
  only the painted area is regenerated; leave the prompt blank to remove what
  you painted. The line above the button says which area it will read. A blank
  prompt without strokes, another kind of card, or a running job disables it.
  The model sees this card's pixels under its current sliders and masks. Each
  adjust costs one gpt-image generation and lands as a new **✨** card at
  `./out/<stem>.adjust.png`, then `<stem>.adjust-2.png`, and so on; the source
  card stays as it is. Crop and straighten are not carried over; set them on
  the new card.
  Adjusts can chain, and **Reverse-fit** reads the result just like any other
  generated card. Whole-image results report structural divergence **D** against
  the input actually sent. Reimagine continues to read the photo's negative.

Local denoise and segmentation do not need an API key. Their Python sidecars
resolve relative to the installed program tree, and downloaded weights are
kept in the local cache rather than committed to the repository.

## Privacy, trust, and paid-feature boundary

| Runs locally without an API key | Uses the configured vision/generative API role |
|---|---|
| Deterministic render and manual develop, including `apply` | Full vision-backed `analyze` / `auto` proposals and visual model review |
| Local `match` inverse rendering | `match --style-prompt`, `--ai-judge`, or `--deep` |
| XMP read/write, masks, curves, and GUI sliders | Generative `reimagine` / `retouch` |
| AI denoise (DRUNet on the RAW mosaic, SCUNet on baked sources) and local BiRefNet/U²-Net, OneFormer, and SAM masks | Automatic target detection in `heal`; a supplied mask works offline |
| Style indexing and retrieval | |

Without the vision role, the advisor can fall back to its disclosed histogram
heuristic, which is not equivalent to the full vision-backed feature. The
data-only verifier defaults to the signed-in `claude` CLI over OAuth, so it does
not require an API key, although provider-backed operations may still consume a
subscription or incur charges.

Photos leave the machine only for AI operations the user requests through a
configured provider. The verifier receives recipe, EXIF, histogram, clipping,
and rationale data—not pixels—and Responses request bodies set `store:false`.
The local web UI binds to loopback only, checks Host/Origin and cross-site
requests, requires a fresh per-run session token for state changes, disables API
caching, and denies framing. By default, AutoShade keeps the source library
read-only. If the configured Delivery folder is inside or above a photo's
folder, that delivery subtree is intentionally writable; Settings warns when
this removes the folder's protection. "Export .xmp beside the photo" is the
separate, confirmed per-photo sidecar exception.

## Install, upgrade, and uninstall on Windows

`AutoShade-Setup-<version>.exe` installs for the current user, needs no
administrator rights, and puts the program in
`%LOCALAPPDATA%\Programs\AutoShade` unless you choose another folder. Two
tasks are offered and both start unticked: a desktop shortcut, and adding the
install directory to your user `PATH` so `autoshade` works from any new
terminal (already-open terminals keep the environment they started with).

**Upgrading.** Run the newer installer; there is nothing to uninstall first.
The welcome page tells you which version it found and which one it is about to
put there. It installs into the SAME directory as the existing install, even if
that is not the default one, replaces every program file, and leaves one entry
in Programs and Features, one `PATH` entry and one set of Start Menu shortcuts
rather than a second copy of each. Two things it does not touch: the model
weights under `python\weights` inside the install folder, which are a
multi-gigabyte download the AI sidecars fetch on first use, and your develop
store in `%LOCALAPPDATA%\autoshade`, which holds your edits, thumbnails and
style index. If AutoShade is running, setup closes it before replacing its
files and does not start it again afterwards.

Running an OLDER installer over a newer install is refused, with a message
naming both versions. Uninstall first if you really mean to go back.

**Uninstalling.** There are two ways in, and they do the same thing: the
AutoShade entry in **Settings → Apps → Installed apps** (Programs and
Features), or 「Uninstall AutoShade」 in the AutoShade Start Menu
folder. Either one removes the program files, the shortcuts, the `PATH` entry
it added and the registry entry — and then asks one question: whether to
delete the downloaded model weights and your develop store as well. The
question names how large each one is, and **No, keep them** is the default
button. Keep them if you might install AutoShade again: the weights are a large
download and the store is your work. The install folder is left in place when
you keep them, because it is where the weights live; it is removed entirely
when you delete them.

**Silently.** For a scripted install, upgrade or rollout:

```text
AutoShade-Setup-<version>.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART
AutoShade-Setup-<version>.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /DIR="D:\Apps\AutoShade" /TASKS="addtopath"
```

The first form upgrades an existing install in place; `/DIR=` and `/TASKS=` only
matter for a first install. A refused downgrade exits with a non-zero code and
writes the reason to the log; add `/LOG="path"` to keep one.

To uninstall without any window, run `unins000.exe` from the install directory:

```text
unins000.exe /VERYSILENT /SUPPRESSMSGBOXES
unins000.exe /VERYSILENT /SUPPRESSMSGBOXES /DELETEDATA=1
```

The first keeps the model weights and the develop store — the same answer
the dialog defaults to. The second deletes both and removes the install folder
with them.
