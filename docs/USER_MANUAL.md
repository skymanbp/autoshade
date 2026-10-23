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
real 0. Since v1.6.5 the Sharpening slider is calibrated against Lightroom's on
a measured frame: the same number sharpens about the same, light edges and
star cores lift the way Lightroom's do, the dark ring around a bright star
digs as deep as Lightroom's and no deeper, and deep shadows are left alone.
The two noise reductions are AutoShade's own, built from what Adobe documents
about each slider — close to Lightroom, not identical.

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
aligned columns. The controls panel keeps the width you drag it to — opening
a fold never widens it — between 280 px and the width at which its column is
the prompt fields' 420 px readable ceiling, so every row and fold ends on the
same right edge. On Windows the toolbar is also the window's title bar: drag
its empty space to move the window, double-click it to maximize, and use the
minimize / maximize / close buttons at its right end; the window's edges
resize it. A slider is laid as in Lightroom: its name on the left and
its value on the right of one line, a thin rail the width of the column under
them; double-click or right-click the rail or the name to reset it, hover it
and press ↑/↓ to nudge it, or click the value to type one. **Color Grading**
is drawn as Lightroom's colour wheels: **3-way** shows the midtone wheel over
the shadow and highlight wheels, **Global** one wheel for the whole picture;
the angle is the hue, the distance from the centre the saturation, Shift
drags finely and a double-click returns a wheel to neutral, with each region's
Luminance under its wheel and Blending / Balance under all three. **Lens
Blur** blurs a photo by distance, as a wide-open lens would: **🤖 Estimate
depth** runs a local depth model (MoGe-2; the first run downloads about 1.3 GB)
over the photo, then **🎯 Pick focus** and a click on the image choose what
stays sharp — everything nearer and farther blurs by how far it is from that
distance, more the farther. **Blur amount** sets the strongest blur and
**Focus range** how deep the sharp zone is; 👁 mutes it and 🗑 removes it. The
blur renders on the canvas and in every export from this app, but a Lightroom
sidecar has no place for it, so the save line names it; it is not pasted onto
other photos (each needs its own depth), and a photo with a lens blur cannot
be rotated until it is removed. The side panel
reads in three levels: a
panel title (**AI generation & reverse-fit**, **AI analysis & auto develop**,
Develop, Retouch) in large type over a short gold rule, group
heads as small spaced grey captions (in Develop, **Tone & Colour** and **Detail
& Lens**), and the folds under them, each opened by a
chevron and drawn as a card while open; groups are set apart
by space rather than lines. A glyph in front of a label always means
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
merged pixels live in its 16-bit master in the masters folder (Settings →
**Develop store** names it and opens it).

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

A **◭ Reverse-fit** card develops the negative the fit started from. The
Reverse-fit fold names both ends: **From** is a menu of the ▣ Original card and
every **◈ Denoised negative** / **▦ Stacked** card, and **To** is the ✨ card
you stand on (or the reference file you picked). **Automatic**, the default,
is the ◈ / ▦ card you stand on, else the first one in the strip, else the ▣
card — its own in-place master after a heal or clone on it, else the loaded
file. The fit is solved on that negative, the ◭ card renders and exports from
it, and the fit's save links it in `pixels.json` so a reopen restores the same
pixels.

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
exactly those shapes into Generative Fill's painted area and re-runs the generative
model over them (an empty prompt removes; the result lands as a layer on the
card, like any other fill). One thing to know when you save: a merge into an existing
`.xmp` keeps your original removal block untouched, but a sidecar written where
none existed carries no removals at all, and the save line names that.

Each of Generative Fill, Heal and Clone Stamp carries its own **🖌 Paint area**
/ **Clear area** row and its own painted area — what you paint for one tool is
not what another reads — and the **Brush size** slider appears under whichever
brush is armed. Every brush shows as a ring of its own size in place of the
pointer, inks the moment the button goes down, and flashes its size for a
second when `[` / `]` (each press about 15 %) or the slider changes it; the
radius runs from 4 to 200 screen pixels. **Feather** and **Flow** under Brush
size are shared by every brush: Feather softens the outer part of the disc (an
inner ring on the canvas marks where the fade starts), Flow is how much one pass
lays down so a low flow builds up as you go over the same place; their defaults,
0 and 100, are the hard full-strength brush of earlier versions. A generative fill does not touch the card's pixels. The model is shown the active
card's developed picture — its sliders and masks applied, in the uncropped
frame the brush paints in — the painted area is regenerated (an empty prompt
removes it), and the result lands as a **layer** over that card's develop: the
card, its crop and its ▣ negative stay as they were, and the fold lists the
card's layers under its button, each with 👁 to hide it, ✕ to remove it and an
opacity slider. Layers are saved with the card (Ctrl+S) and drawn on every
export; Ctrl+Z takes the last one off. The model redraws the whole frame a
little brighter or darker than it was sent, so before the regenerated area is
pasted it is levelled by how far the model moved the parts it was meant to
leave alone, and its edge meets the colours around it. With nothing painted,
the same fold edits from words alone: write what to change ("remove the wires
on the left", "make the sky bluer") and press **✨ AI edit** — the model is
sent the card's picture with no mask, and only the area its answer changed
lands as the layer (the status line says what share of the picture that is).
A layer keeps the look it was made with, so after a large slider change,
fill again. Lightroom has no place for a
layer: the `.xmp` carries the rest of the develop and the save line says the
layers stayed behind. Pasting a develop onto another photo leaves the layers
out (they are this photo's pixels), and turning the photo is off while a card
holds one. Through v1.6.7 a fill landed as a new **✨ AI generated** card
instead. The browser's Fill and the CLI's `retouch` still composite onto the
source's neutral develop.

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
path. It is solved from the same population estimator — since 2026-10-21 the
colour change the frame's brightness ranges AGREE on (each range's own median
of its pixels' colour change with the brightness change taken out; where a
tenth of the frame or more asks the opposite move, by at least a fifth of
the agreed one, there is no one light to correct and nothing ships), read from the frame as the tone stage left it
rather than from the untouched source, so a strong contrast move is no longer
booked as a colour cast, a warm move no longer comes back green, and a bright
sky that asks to stay cool is no longer yellowed by a warm majority below it —
and then RENDERED and
checked against the target's own 12×8 cell means before it ships: if the
render did not move the frame toward the target, the recipe returns to as-shot
and says so. A same-frame pair whose light did not change therefore still
reports `temperature_k` unset. Since 2026-10-21 a white balance the cells
refuse only on its SIZE — most cells asked for the move's direction, and a
tenth or more of the frame was pushed past its own target — is not returned
to as-shot: the largest share of the solved move whose render the same cells
do vouch ships instead, provided the cells that refused the full move mostly
stop refusing at that share (otherwise they were not asking for less of the
move but for none of it), and the sentence prints that share beside the
refused full move's verdict. A frame whose cells want opposite moves stays
refused.

The tone stage reads every paired pixel since 2026-10-21, not only the
near-grey ones: on a street whose grey pixels are the shadow walls the target
crushes, the grey-only map asked for contrast 100 and took the whole frame's
shadows with it. The tone stage and the residual tone curve each answer for themselves too,
on the tonal reading (the luma-distribution term of the look error, the one
thing a tone move controls): rendered, if the tone sliders leave the frame's
tones farther from the target's than the untouched source's, they ship nothing
and the later stages solve from the untoned frame; if the curve on top of the
sliders reads worse than the sliders alone, the sliders ship without it. Each
refusal has its own sentence. Before this the
only such check was the one at the very end, which returned the WHOLE recipe
to neutral — on a lantern-lit street the white balance the cells had vouched
was thrown out with the tone move that had harmed the frame.

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

With **Fit local areas too** enabled, reverse-fit always solves the global recipe
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

An analysis's verdict, rationale and deep-thinking working sit in one box at
the top of the Analysis fold, about eight lines tall; a longer analysis scrolls
inside it and adds **View full text…**, which opens the whole text in a
window you can move, resize and copy from.

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

The following commands and flags match the command definitions in
`src/main.rs` (checked flag for flag against each command's `--help` for
v1.7.0):

```text
autoshade decode <src> [-o|--out FILE]
autoshade analyze <src> [-o|--out FILE] [--guidance TEXT] [--guide library|direction] [--style 0..1] [--strength 0..1] [--adherence 0..1] [--embed|--no-embed] [--deep] [--reference-image]
autoshade apply <src> <recipe.json> (-o|--out) FILE [--long-edge N]
autoshade auto <src> [-o|--out FILE] [--guidance TEXT] [--guide library|direction] [--style 0..1] [--strength 0..1] [--adherence 0..1] [--embed|--no-embed] [--deep] [--reference-image] [--denoise] [--denoise-strength 0..1] [--denoise-model NAME] [--long-edge N]
autoshade denoise <src> [-o|--out FILE] [--strength 0..1] [--model NAME]
autoshade batch <dir> [--render] [--limit N] [--include-baked] [--jobs N] [--long-edge N]
autoshade eval <dir> [--xmp-dir DIR] [--limit N] [--jobs N] [--fresh] [--state FILE]
autoshade library-build <dir> [--describe] [--raw-root DIR]
autoshade library-search <photo> [--style 0..1]
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
A 16-bit TIFF or PNG master from `stack`, `heal` or `denoise` carries the sRGB
profile an sRGB export carries (v1.6.3): another editor reads its colours as
they are, and reading it back raises no untagged-16-bit warning.

**`match` on a denoised or stacked master (v1.6.2).** `match` fits what the
desktop app fits. A RAW is fitted
on a neutral develop of its sensor frame with the photo's calibration —
camera base look, lens profile, as-shot white balance — composed into the
solve. A master `denoise` or `stack` wrote is a baked image that carries no
calibration of its own, so name the RAW it was made from (for a stack, the
first frame) with `--negative`: the calibration then composes on top of the
master's pixels, exactly as the desktop app fits a ◈ Denoised or ▦ Stacked
card, the fit reads the same structure the RAW's fit reads, and `--render`
shares the RAW's frame. Without the flag a baked source is fitted under the
calibration its own saved develop carries, composed the same way — the
desktop app's rule for a baked file. For a file nothing has calibrated that
is none, and it is fitted as it stands: on the reference pair a bare
denoised master read its sky at a structural divergence of 0.716 against
the RAW's 0.649 — across the 0.65 line into the bounded atmosphere solver —
and rendered without the lens profile. A master fitted once with
`--negative` keeps its RAW's calibration in its saved develop, so a later
`match` on it without the flag fits under that calibration too. A RAW
source refuses the flag: it is its own
negative, and `match` on it describes the RAW itself, which is what its
saved develop and its Lightroom sidecar render afterwards.

### The reference library — `library-build` and `library-search`

`library-build <dir>` measures every finished photo under `<dir>` (searched
recursively) with the local SigLIP 2 model and writes `reference-library.json`
in your develop store; a rebuild measures only new or changed photos. It ends
with one line: `reference library -> <file> (N finished photos, P with look
phrases, S with settings)`. `--describe` and `--raw-root` are described under
[Configure and use the AI features](#configure-and-use-the-ai-features).
`library-search <photo>` is the offline diagnostic: it prints which references
a library-guided develop of that photo would get, in the search's own numbers,
and never calls a paid model.

### Where your `.xmp` sidecars are — `--xmp-dir`

`eval` pairs each RAW with the `.xmp` sidecar your editor
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

On `analyze` and `auto`, `--embed` lets the local SigLIP 2 sidecar measure the
photo so the reference library can be searched, and `--no-embed` refuses it
(the develop then goes without references); either flag wins over the
environment, and neither writes to it.

`--adherence 0..1` picks the prompt tier the proposer and verifier are told:
`<=0.40` Hint, `0.40..0.70` Direct, above `0.70` Brief, default `0.65`
(Direct). It never moves a render bound. It does nothing without a direction
that guides the develop, which is why the desktop app greys the slider out
until the switch is on **Direction** and Direction has text.

Every setting below is named `AUTOSHADE_*`. Up to v1.1.0 the app was called
Autoshop and these variables were named `AUTOSHOP_*`; the old spelling still
works everywhere, warns once naming its replacement, and is removed in the
release after this one. Where both are set, the `AUTOSHADE_*` one wins.

Three environment overrides steer the reference library, each read in exactly one place:

| Variable | Effect |
|---|---|
| `AUTOSHADE_STYLE_EMBED` | `1`/`0` — use the SigLIP sidecar. Set (any value) beats the GUI preference; `--embed`/`--no-embed` beats both. |
| `AUTOSHADE_STYLE_DESCRIBE` | `1`/`0` — write look phrases with the local model during `library-build`. Set (any value) beats the GUI preference; `--describe` beats both. |
| `AUTOSHADE_SEND_REFERENCE_IMAGE` | `1`/`0` — also send the reference photos themselves (not just their phrases) with `analyze`/`auto` proposals; `--reference-image` turns it on per run. Destination-trust: only your own environment or user-level settings can set it — a downloaded photo pack's `.env` cannot, because it decides whether your photograph goes on the wire. `batch` never sends one. |

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
generative images: any server that speaks that API works from its base URL
and a key. **API style** says which request shape the vision calls use —
**Auto** (the default) tries the Responses API and, when the server answers
404 or 405 there, sends the same call as a Chat Completion and remembers
that until the app closes; **Chat Completions** pins that shape for a
server that serves only `/chat/completions` (most OpenAI-compatible servers
and local runners); **Responses API** pins the original. A server that
accepts the Chat shape but not its structured-output contract gets the call
again without it, and the reply is read as the JSON the prompt asked for.
Generative edits are the exception: by default they use OpenAI's
`/images/edits`, which only some providers serve. **Image-gen API** →
**Custom** describes any other provider's edit call as a JSON template sent
to the same base URL with the same key — `"path"` is added to the base URL,
`"body"` is what is sent, with `{model}`, `{prompt}`, `{image}`,
`{image_data_url}`, `{mask}`, `{mask_data_url}`, `{width}`, `{height}`,
`{size}` and `{quality}` filled in (`{mask}` is white where the picture should
change). A provider with no mask parameter gets
`{marked}`, the picture with the painted area tinted red, as another image,
and `{marked_note}` after the prompt, which tells the model to change only the
red area; that is how an empty prompt (remove) still says where. A lone
painted-area entry is dropped, and the note left empty, when nothing is
painted. `"reply"` is `"image"` when the answer is the picture itself or a JSON
pointer to its base64 (default `/data/0/b64_json`). The empty box shows an
example template to start from; the provider's own API reference says what
its path, field names and reply are, and the **Image-gen model** field holds
the model id that provider uses for edits. An empty template saves as OpenAI
standard. The answer is resized to the requested size when it has the same aspect, and
refused otherwise; either way only the painted area of the answer is kept.
The same template can come from `AUTOSHADE_IMAGE_GEN_ENDPOINT`. Under **Sign in
with ChatGPT** the bridge serves OpenAI's call, so the choice is hidden and a
saved template waits until the API option is chosen again.
**Fetch models** lists what the endpoint
serves; where the listing also says which reasoning tiers a model takes
(Venice, OpenRouter), the **Reasoning effort** picker offers that model's
own tiers, says so beside it, and a model that takes none is marked so that
the field is left blank; a vision model the listing says takes no images is
flagged beside the picker. The verifier defaults to the signed-in
`claude` CLI over OAuth, receives statistics and recipe data rather than
image pixels, and can instead use an API provider.

**Sign in with ChatGPT.** The image role's other option uses a ChatGPT
subscription through a Codex bridge, CLIProxyAPI, that you install and run
yourself: download it from its GitHub releases, start it, sign in with your
ChatGPT account when it asks, and paste the `api-keys` value from its
configuration file into the **Bridge password** field. The bridge must be
running before an edit, or the call cannot connect. Generated images are
capped at about 1.5 MP by the subscription; for full-resolution edits use
the API option with a real key.

The same roles can be configured from the environment: `OPENAI_API_KEY` serves
the image/vision and generative role; `AUTOSHADE_ANALYSIS_API_KEY` is used only
when the verifier is set to API mode. Settings are saved in the per-user
`autoshade.local.json`; do not put real credentials in the repository. A
`./autoshade.local.json` in the current working directory may select
model/provider preferences but cannot supply API credentials, endpoints,
executable/script paths, or output destinations, so an opened photo folder
cannot become a credential or path override.

**Theme.** Settings also picks the desktop app's look: **Dark**, **Light**,
**Warm ink** or **Cool blue**, and an **Accent colour** (gold, teal, coral,
violet or green) for the main buttons and highlights. The choice is saved with
your preferences and applies at once.

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

The side panel keeps the two AI paths apart: **AI generation & reverse-fit**
(Reimagine, Adjust generated image, Reverse-fit) and **AI analysis & auto
develop** (Analysis, Adjust the selected variant, Reference library).

- **Analyze:** choose **AI Analyze** under **AI analysis & auto develop** or
  run `autoshade analyze`.
  The vision advisor proposes bounded sliders and masks, a data-only verifier
  checks the proposal, and normal visual review may attempt one revision;
  `--deep` permits additional bounded rounds. Accepted output remains a normal
  recipe and XMP. The model may set every develop control it can judge from
  the preview — tone, white balance, presence, the colour mixer, colour
  grading, the curves, sharpening and noise amounts, the manual lens trio,
  crop and straighten, local masks, and (since 2026-10-21) the post-crop
  vignette, grain, the Black & White treatment and the Calibration panel; a
  control it has no opinion on keeps your value. It never sets the sharpening
  radius/detail/mask, the noise-reduction shaping axes, chromatic aberration,
  de-fringe, Transform, HDR or the profile names: those are either structure
  its preview cannot show or the photographer's own setup.
- **Adjust the selected variant · paid API:** the fold under Analysis. Type
  what to change on the card you are looking at ("darken the sky a little,
  warm the faces") and click **🤖 AI adjust**. The model sees the card as it
  looks now and its current recipe, and returns the complete recipe that
  replaces it, using every develop control; a part it names — the sky, the
  subject, an object — is selected by a content mask the local segmenter
  cuts after the answer (an approximation of that part, not a hand-drawn
  selection, and the rationale says so), with linear, radial and range masks
  for the rest. It lands like an analysis: into the sliders, undoable, and
  as a new **✎** card when the selected card is a pristine **✨** one; a card
  on AI-generated pixels is adjusted from its own picture. Same chain and
  cost as Analyze, plus a few seconds of local segmentation per content mask.
- **Reference library:** pick a folder of finished photos whose look you like
  (JPEG, TIFF or PNG exports — yours or anyone's) under **AI analysis & auto
  develop › Reference library**
  or run `autoshade library-build <folder>`. Each photo is measured from its
  pixels; the finished photos' own `.xmp` files are never read. Optionally
  pick a **RAW folder** (or pass `--raw-root DIR`): each finished photo's RAW
  is then found by its file name (`frame0264-final.jpg` → `frame0264.ARW`), the RAW's
  `.xmp` is developed, and only when that develop looks like the finished photo
  (mean colour difference ΔE ≤ 11) do its settings go to the model beside that
  reference. A sidecar that develops to a different picture — an older grade,
  or one later reworked in Photoshop — is ignored. Tick **Write look phrases** (or
  pass `--describe`) to have a local model write a few phrases per photo about
  its grade, which go to the model beside each reference. A rebuild measures
  only new or changed photos. Each analysis guided by the library then
  searches it and gives the model the 3 closest references (versions of one shot —
  an export and its re-edit — count once) as words, and — with
  **Also show the model the reference photos** — as pictures, one extra image
  each on every call. The **Style** control sets how closely they are followed:
  below 85 % they are a ceiling, at 85 % or above a target, and at 0 the library
  is not read (only the two switches that feed an analysis go grey; building
  stays available). `autoshade library-search <photo>` shows which references
  a develop would get, with no paid call. Strength independently controls the
  fit budget and confidence cap; Direction adherence chooses Hint, Direct, or
  Brief wording when a Direction guides.
- **Library or Direction — one or the other:** an analysis is guided EITHER
  by your reference library OR by the Direction you type, never both. With the
  library, the Direction text is not sent; with the Direction, the library is
  not read and the Style control does nothing. The judge that reviews the
  finished frame is briefed on the same one aim.

  All three surfaces choose. Desktop: the **Guided by** switch above Direction
  (*Reference library* / *Direction*); the side not chosen is greyed out, and
  the choice is remembered. CLI: `--guide library|direction` on `analyze` and
  `auto`; omitted, a `--guidance` picks the direction and no `--guidance` picks
  the library, and `--guide library` together with `--guidance` is refused.
  Browser (`autoshade serve`): the **Guide** menu beside Direction, and for
  anything driving that HTTP API directly, the `POST /api/analyze` body takes an
  optional `guide` field (`"library"` or `"direction"`) resolved the same way,
  plus an optional `adherence` (0..1, default 0.65). A box dragged on the image
  is a local direction, so it always guides by the direction.
- **Reimagine:** enter a prompt under **AI generation & reverse-fit** or use
  `reimagine` to create a
  generated, lower-resolution target. `--fidelity high` (the default, and the
  GUI's mode) tells the model to re-develop the same photograph, not repaint
  it. The structural divergence **D** against the sent input is disclosed;
  `D ≥ 0.35` warns that a reverse-fit of that result will fall back to
  Atmosphere mode, and the opt-in `--fidelity-retry` (a GUI checkbox as well —
  off by default, it buys a second image) regenerates once and keeps the
  closer result. Use **Reverse-fit** or `match` to infer a deterministic recipe
  and apply it to the original RAW at full resolution.

- **Adjust generated image · paid API:** the fold directly after Reimagine
  under **AI generation & reverse-fit** edits the selected **✨ AI generated**
  card or its **✎** edit.
  It has its own prompt, remembered high/medium/low quality, and its own
  **🖌 Paint area** / **Clear area** pair: every tool that takes a brush
  (Generative Fill, Heal, Clone Stamp and this fold) keeps a painted area of
  its own, and only that tool's verb reads it. With no strokes, enter what to
  change (for example, "make the sky bluer") and click **✨ Adjust** to edit
  the whole image. With strokes in this fold's area, only the painted area is
  regenerated; leave the prompt blank to remove what you painted. The line
  above the button says which area it will read. A blank
  prompt without strokes, another kind of card, or a running job disables it.
  The model sees this card's pixels under its current sliders and masks. Each
  adjust costs one gpt-image generation and lands as a new **✨** card at
  `<stem>.adjust.png` in the masters folder, then `<stem>.adjust-2.png`, and so on; the source
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
| AI denoise (DRUNet on the RAW mosaic, SCUNet on baked sources), local BiRefNet/U²-Net, OneFormer, and SAM masks, and the MoGe-2 depth map behind Lens Blur | Automatic target detection in `heal`; a supplied mask works offline |
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

The Delivery folder holds exports only. The full-resolution images AI denoise,
stacking, heal, clone, fill and AI edits produce — the masters their cards are
built on — live in a `masters` folder inside the develop store, so cleaning out
the Delivery folder never breaks a card. Settings → **Develop store** shows
that folder and opens it; delete a card before deleting its master. Masters
made before v1.12.0 stay where they were written. If one of those is deleted,
a ▣ Original card that used it goes back to the photo file itself and says so,
and any other card built on it has nothing left to show and can be deleted.

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
reference library. If AutoShade is running, setup closes it before replacing its
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
