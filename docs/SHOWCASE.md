# AutoShade showcase

The three figures behind the README's pillars and the frames on
[autoshade.dev](https://autoshade.dev/#showcase-a), with every number the
captions quote and the prompts that bought the generated targets. All three
were produced on 2026-09-01 — Part B on the v1.2.2 build, Part A on the
pre-release tree as its caption says; every frame not marked
*generated* is rendered by AutoShade's engine from a recipe, and every
"straight conversion" is AutoShade's own neutral develop of the RAW, not the
camera JPEG. Model-judge scores are automated review, not human aesthetic
approval. Image paths are relative to this file's directory.

Every RAW is a Sony α7R IVA 61 MP `.ARW` (9504 × 6336). The Cornwall frame was
shot with the body set to a 4:3 aspect, which is how it surfaced the two
defects v1.2.2 fixes on the way to this page (see
[RELEASE_NOTES_v1.2.2.md](RELEASE_NOTES_v1.2.2.md)).

## Part A — AI analysis with a style reference: one photograph, four looks

<img src="images/showcase-island-four-looks.jpg" alt="Lakeside island town: straight conversion and three AI develops driven by three different direction texts" />

<sub><b>Lakeside island town.</b> Top left, the straight conversion of a hazy
frame. The other three are AI develops of the same RAW at the same
<code>--style 1.0 --strength 0.9</code> against the same index, and the only
thing that changes between them is the <b>direction text</b>. That index held
the photographer's 94-photo finished-look library and <i>no</i> RAW+XMP
exemplars — the state v1.2.2's Defect 3 had left every index built on
v1.2.0/v1.2.1 in, and the state this library was in when the panel was
rendered, on the pre-release tree before the loader fix landed (the v1.2.2
binary reads the same index file to the same state: 0 exemplars, 94 finished
photos). So the Style axis had nothing to act on — each run's rationale says
so in as many words — and what the model received was the direction text and
one finished photo, chosen from the look library by that text, as an image.
The direction led: mean saturation 34 % / 12 % / 29 % (moody / golden /
vivid) against the conversion's 17 %, mean brightness 38 % / 61 % / 65 %
against 47 %, measured on the panel's cells. Direction and judge trail, per
panel: <i>dark moody low-key tones, a cross-processed colour treatment, a
teal-and-orange split tone</i> — visual judge 71/100 Revise (the village
crushed toward black, the orange counterpoint too weak), its guided revision
re-scored 64 and was discarded, verdict Accept; <i>warm golden tones,
film-like grain, lifted matte shadows</i> — the verifier sent the proposal
back twice because it never set the grain the direction asked for, the visual
judge scored 84 and its guided revision 92 was adopted, and the verifier's
word on the adopted recipe was Revise, so this panel is the unsaved proposal
rendered with <code>--out</code> (the proposal also crops 3 % off the top and
5 % off the bottom); <i>vivid saturated colours, punchy high contrast, crisp
clarity</i> — visual judge 64/100 Revise, guided revisions re-scored 72 and 84
and were adopted, a third 73 was discarded, verdict Accept. For contrast: with
the photographer's own 169 Lightroom RAW+XMP pairs indexed beside the look
library (SigLIP 2 image vectors + Qwen3-VL descriptions), the same three
directions at the same settings came back at 23 % / 11 % / 18 % saturation
and 54 % / 58 % / 55 % brightness — inside the photographer's own cool, hazy
register, because <code>--style 1.0</code> states the four nearest edits'
habits to the model as the target and the direction then moves within that
anchor. That is what the Style axis is for; a tier in which a given direction
leads and the photographer's edits become background is registered for
v1.2.3. Nothing is copied pixel for pixel: the reference reaches the model as
one image and the direction as text, and what comes back is a recipe the
engine renders.</sub>

## Part B — Reimagine → reverse-fit: the recipe carries the look, the RAW carries the detail

Part B is a different workflow: buy a complete visual target from a
generative model, then fit an ordinary engine recipe to its look. The
generated target can invent content; the fitted render cannot — it is the
original RAW through a recipe, so it has every pixel the sensor recorded. Each
panel's bottom row is the same small window of the frame from each stage at
its own native resolution: the straight conversion and the engine render are
1:1 crops of the 9504-px frame, the generated target is the same window of
its 3520-px frame, upsampled.

### Stone viaduct

<img src="images/showcase-viaduct-reverse-fit.jpg" alt="Stone viaduct: straight conversion, generated target, and the recovered recipe rendered on the RAW, with a 1:1 detail row" />

<sub><b>Stone viaduct.</b> Target bought from a configured
<code>gpt-image-2</code> at 3520 × 2352 with the prompt <i>"the same
photograph on a clearer afternoon: a little more contrast and a slightly
deeper blue sky, everything else unchanged"</i> — a grade, not a rebuild,
which is what keeps a generator on the input's structure: <code>reimagine</code>
measured <b>D = 0.177</b> against the frame it sent, and the calibration test
that pins this panel re-measures the two top-row frames at
<b>D = 0.180</b>, under the 0.35 threshold, so the full solve ran.
Look error <b>0.161 → 0.050</b>: a global tone and saturation solve, the
per-band colour mixer on four bands (Orange −18/+7, Yellow −18/−5, Aqua
+18/−18, Blue +18/−18 saturation/luminance), a sky zone (−0.15 EV) and a
land zone, four boundary-gated spatial tiles and two field masks. Confidence
0.25, because the target's look also uses controls the fit never solves
(per-band hue, colour grading, local masks), and the rationale says so. The
sky tile at top left is the one the v1.2.2 seam fix was measured on: its
cross-boundary step reads 0.0278 → 0.0042 at k = 0.121 (context-charged
0.0116 against the 0.012 ceiling), and the delivered seam on the mask-free
ruler fell from +3.15 to +0.92 codes.</sub>

### Cornwall lighthouse islet

<img src="images/showcase-cornwall-reverse-fit.jpg" alt="Cornwall lighthouse islet: straight conversion, generated target, and the recovered recipe rendered on the RAW, with a 1:1 detail row" />

<sub><b>Cornwall lighthouse islet.</b> Target bought from
<code>gpt-image-2</code> at 3520 × 2336 with the prompt <i>"the same
photograph with a slightly deeper blue sky and a little more contrast,
everything else unchanged"</i>. This body was set to a 4:3 aspect, so its
embedded preview is a centred 4:3 crop of the 3:2 sensor: the first purchase
sized the request from that preview, sent the develop squashed into 4:3 and
measured <b>D = 0.304</b>; sized from the sensor frame (v1.2.2) the same
prompt measured <b>D = 0.139</b>, and the panel's top-row frames re-measure
at <b>D = 0.136</b>. Look error <b>0.137 → 0.027</b> at
confidence 0.65, fitted on a neutral develop of the full frame
with the calibration composed into the solve — the route the desktop app
takes, which the CLI now takes too whenever the preview is not the sensor
frame: a global tone and saturation solve with the per-band mixer on Aqua and Blue (−18 luminance each), a sky zone (−0.10 EV, colour gains 0.96 / 0.99 / 1.09, saturation +8; zone residual 0.067 → 0.008) and a land zone (−0.15 EV, gains 1.12 / 0.97 / 0.96, saturation −9; 0.036 → 0.003), four boundary-gated spatial tiles (r0c3, r0c0, r2c2, r0c1, each context-charged 0.0110–0.0120 against the 0.012 ceiling) and two field masks (frame 0.0291 → 0.0269). The global stage also admitted per-channel cast curves — red 0 → 23 and 255 → 179, green 0 → 56 and 255 → 189, blue 0 → 50 and 255 → 209 — which pass the fit's re-hue gate (a region of ≥ 5 % of the frame rotated ≥ 75°) yet tint the delivered sky toward violet against the target's blue. The render is shown as fitted, with that tint; the gap — a hue-preserving cast stage, and a rationale note on ordinary cast admission, which today is silent — is registered for v1.2.3 in the roadmap.</sub>

## What the figures do not show

Reverse-fit recovers global tone, saturation and guarded colour casts, and
bounded local corrections behind evidence and boundary gates. It does not
claim to recover generated objects or detail, per-band hue rotation, or a
target's local masks; every recipe's rationale names what the target appears
to use that the solve cannot reach. The style read carries a mood — with the
photographer's own edits indexed the advisor sees the four nearest as numbers
and prose, and in either case one finished photo as an image — not a
pixel-level look transfer, and a run that ends on a *Revise* verdict saves no recipe.
