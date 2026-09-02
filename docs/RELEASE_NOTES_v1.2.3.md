# AutoShade v1.2.3 — a fan the hue gates could not see, and who leads when you write a direction

Three findings from the v1.2.2 showcase, each fixed at its cause and each
measured on the frames that surfaced it. The Cornwall reverse fit had shipped a
colour cast that passed every hue veto the stage had and still turned one blue
sky violet at the top and blue at the horizon: three independent channel curves
can fan a single hue class across luminance without moving its mean, and no
gate asked that question. The island four-looks panel showed the Style axis
winning every argument with a written direction, because the library was the
only thing the reference block and the blending arithmetic knew how to aim at.
And the luminance-range boundary budget — the second finding the v1.2.2 seam
ruling named and did not act on — is now measured on both real pairs and a
synthetic ramp, and kept, with the record corrected where it had been written
from the wrong basis.

Also in this release: the showcase panels are re-rendered on this build with
the photographer's full index; the browser page gains the Adherence dial the
other two surfaces already had; four rationale notes that v1.2.2 shipped
untranslated reach the Chinese UI; and the calibration-corpus tests run in the
release battery instead of silently returning.

## Defect — a pale sky re-hued under the 75° census

The Cornwall lighthouse-islet panel shipped in v1.2.2 "as fitted", with the
tint registered rather than corrected: its global stage admitted per-channel
cast curves that pass every hue gate the fit had and still turn the delivered
sky against the target's blue. The registered gap was "a hue-preserving cast
stage, or a tighter admission". The first thing this round did was measure why
the curves passed, and the answer moved the fix: the gates were not too loose,
they were asking a question that cannot reach this damage.

**What the curves actually do.** All three cast vetoes ask about a pixel's
DESTINATION — how far it travelled (rotation budget, ≥ 75° over ≥ 5 % of the
frame), whether it landed in a hue the target holds nowhere (foreign-hue
veto), whether the aggregate improved (ratio). Three *independent* monotone
channel maps do the one thing none of those questions reach: they sort a
single-hued region into several hues **by luminance**. Measured on the
analysis raster the fit itself uses, the admitted Cornwall curves

- move the sky's mean hue **218.3° → 217.6°** — 0.7°, invisible, and a
  circular-mean test is blind to it because the slices rotate in *opposite*
  directions and cancel;
- rotate **no pixel at all** past 75° (weighted re-hued share `0.000000`,
  unweighted 0.0058);
- create **0.000000** foreign-hue share;
- cut the look error nearly in half, 0.0576 → 0.0334 (ratio 0.580) — so the
  ratio gate has nothing to object to either;

and split the sky's hue **across luminance** from a 1.6° spread to **33.1°**
in the delivered render: 226.8° (violet) in the dark half, 193.8° (green-cyan)
in the bright cloud. The source sky, the no-cast fit and the target sky all
hold their hue within 1.6° across luminance octiles. The picture the caption
called violet is that fan.

**Root cause.** Not a threshold. The census that guards this stage is
per-pixel and destination-shaped; a fan is a *relational* property of a
region — its slices' hues relative to each other — and no aggregate over
single pixels can see it. Per-channel curves are the only control in the
solve space that can produce it, which is exactly why the gate belongs to
them.

**The fix — a fourth veto, the hue-fan gate.** It re-reads the rotation
budget's *exact* census population (a measurable hue before, chroma ≥ 0.03; a
visible tint after, chroma ≥ 0.04; evidence-weighted), bins it by 15° hue
class and by the evidence model's own luma bins, and measures the widest
circular gap between a class's slice mean hues **minus the gap they started
from**. The subtraction is what makes it a capability gate and not a second
rotation budget: a class that was already fanned (content) contributes
nothing, and a class the curves rotate *rigidly* — a real global cast
correction, every slice moving together — reads zero however far it moves. A
class holding ≥ 5 % of the population that gains ≥ 15° of spread is refused,
and the refusal is disclosed with the class share, the fan and the limit.

On Cornwall the convicted class holds **0.917** of the census population.
That number names a *hue population*, not a region: it is the seascape's
whole blue class — sky **and** sea — which the three curves sort by luminance
together. The row-defined sky alone carries 0.561 of the hue weight.

The threshold is measured, and it was verified end to end rather than on the
census alone. Readings on every calibration pair: the wreck **37.6°**, its
synthetic fixture **44.6°**, against the accepted haze correction **7.8°**,
canyon-warm 7.5°, canyon-gold 5.2°, hazy→vivid 2.7°, an identical pair 0.0°.
15° is one full hue class of the census's own grid, 1.9× above the largest
legitimate reading and 2.5× below the wreck. At a 20° threshold the Cornwall
solve does not refuse outright: the mixer's do-no-harm loop halves Aqua/Blue
and refits until a milder cast measures 19°, which ships and still leaves a
20.6° fan in the delivered sky (the violet gone, a pale green in the bright
cloud not). At 15° the refusal stands.

**What the gate does NOT promise.** Three limits, stated here because they
are the shape of the fix and not footnotes to it.

1. **It is a calibrated threshold, not a structural guarantee.** The mixer's
   do-no-harm loop re-fits the curves after every shrink, so the gate behaves
   as a budget the solve can search against, not only as a veto — the 20°
   experiment above is that behaviour caught in the act, a 19° cast walking
   in under the limit. 15° was chosen *with* that end-to-end failure as its
   evidence, not from the census readings alone.
2. **The admitted worst case is ≈ 2 × 15°.** The class is a 15° bin of the
   BEFORE hue, so the baseline the census subtracts is itself bounded by one
   class width — and one class width is 15°. An admitted cast can therefore
   leave up to **30°** of *absolute* in-class hue spread in the delivered
   frame. That bound is asserted, not merely stated
   (`an_admitted_cast_delivers_at_most_two_class_widths_of_hue_fan`): on the
   admitted haze pair the measured figures are 7.8° added and **11.8°**
   delivered, well inside it.
   Relatedly, the 15° bins have a fixed phase: a coherent region straddling a
   class edge splits in two and can fall under the 5 % floor. The foreign-hue
   veto has always read hue on this same fixed grid, and keeping one identical
   census population is what stops the two gates drifting into disagreeing
   about which pixels they judge.
3. **The deliberate cost is real, and now it has a fixture.** A scene
   genuinely lit by two sources of different colour temperature *needs*
   different hue movement at different luminances, and this gate refuses that
   correction too — the fit then under-corrects with tone, saturation and the
   per-band mixer. No such *photograph* is in the calibration corpus, so the
   size of that cost on real work is still unmeasured; but the case is drawn
   as `two_temperature_coast()` (the coast geometry with the sky's hue riding
   its own brightness, ±25°, a 50° fan in the target itself) and pinned by
   `a_projection_that_cannot_clear_the_target_leaves_the_refusal_standing`.
   It is also the one case the projection below cannot rescue, and the test
   is what says so.

## …and a projection, so a convicted cast is shrunk instead of thrown away

Refusing outright cost the showcase pair a third of its fit, and the fan
gate's verdict is narrower than "this cast is wrong" — it says *not in this
shape*. So when the fan gate is the ONLY gate that fails (both pixel-aligned
vetoes and the ratio gate all clear), the stage no longer
empties the curves. It walks them down a one-parameter path and ships the
strongest point that clears.

The path gives up the **chromatic** part first. With `L` the per-knot mean of
the three fitted outputs — the shape all three channels share, one curve
applied to every channel — and `dC = C − L` each channel's deviation from it:

```
C(t) = x + min(1, 2t)·(L − x) + max(0, 2t − 1)·dC
   t = 1    the fitted cast
   t = 0.5  one shared curve, no chromatic difference between channels at all
   t = 0    no curves
```

It is still three Lightroom RGB curves at every `t`, so the recipe
round-trips to XMP unchanged, and it is the same halve-and-refit idiom the
mixer already uses one stage up.

**The lower half of that path is a deviation from the design, and the
measurement is why.** The design stopped at `L`, on the premise that one
curve applied to all three channels cannot fan a hue class. The premise is
false, and the showcase pair is where it fails: hue is a *ratio*, so a shared
curve moves it wherever its slope changes, and Cornwall's shared shape has
segment slopes 0.172 / 0.859 / 1.127 / 0.188 — its top segment nearly flat
because the fitted red curve clips at 179 from input 191 up. On that shape a
dark sky pixel moves +0.2°, a mid one −3.2°, a bright one **−20.1°** as its
blue channel is crushed toward the other two, and the census reads **17.3°**
of added fan at `t = 0.5` — above the 15° refusal line itself. A family whose
mildest member is still convicted rescues nothing, so the path continues to
the identity, where the fan is zero by construction and the outcome is
exactly the old refusal.

The search is a 12-step bisection for the largest `t` whose **rendered**
candidate is admissible — all four gates and the 7.5° target, never an
algebraic reading of the curves — with the gain bar then applied once to that
winner, and every
candidate is re-judged by all four gates from scratch, with the strength
budget's own bound riding into that judgement exactly as it does for a fitted
cast. It runs only when the fan gate is the **only** gate that convicted: a
pixel-aligned veto says the *destination* is wrong and no point on the path
makes a wrong destination right, and the ratio gate says the curves did not
buy enough, which a weaker version of the same curves cannot answer either.
Both keep the refusal they already had. Two thresholds are the projection's
own:

- **It must clear half the refusal line, 7.5°, not 15°.** 15° is where the
  calibration put the visibility edge (the 20° experiment above shipped a 19°
  cast that left 20.6° of delivered fan), while the widest fan the gate
  passes on its own merits is the haze correction's 7.8°. A cast the fit
  *chooses* to keep must be no worse than one it already accepts. Both
  targets were measured end to end before the constant was fixed — see the
  table below.
- **It must buy more than `FIT_QUANT` (0.0018) of absolute look error** — the
  fit's own quantisation budget, the same constant the terminal do-no-harm
  check uses. The four gates decide whether a cast the fit *measured* may
  ship; whether a milder one the fit *invented* is worth shipping is the
  projection's own question, and marginal gain does not earn regional risk.

The two are applied in different places, and that is a soundness requirement.
A bisection can only find the largest member of a *downward-closed* set, and
only the first half is one: the fan grows with `t` and every gate clears as
the curves approach the identity, while the gain falls to zero at `t = 0`,
where there are no curves at all. With both inside the loop the clearing set
is an interval `[a, b]` with `a > 0`, so a probe that fails on the **gain**
pushes the search away from the band and out at "no rescue" — and the refusal
sentence then claims the whole path was searched by a search that never looked
at it. So the loop tests admissibility alone and the gain bar is applied once,
to the winner. The gain is not monotone in `t` (measured on the coast fixture:
0.00104 at `t` 0.25, 0.00190 at 0.35, 0.00169 at 0.40, 0.00187 at 0.50 — a
wiggle the size of `FIT_QUANT` itself), so judging only the winner refuses two
marginal rescues the older shape happened to find. That is the right
direction: a rescue whose worth depends on which point of the path you land on
is worth nothing. The cost is stated, not hidden: the gain is evaluated at exactly ONE point,
the strongest admissible shrink, so a shrink that pays only at a milder `t`
is not found — measured on the two-family HSL pair (2026-09-02), where every
`t ≤ 0.25` is admissible and pays 0.0019–0.0033 while the strongest
admissible point reads −0.012, so the pair is refused and its refusal
sentence says exactly that. A best-paying-admissible search is a registered
follow-up, not this batch's.

**What a projected cast tells you** is at least what an admitted one does, and
for a sharper reason — these are curves the fit *invented* to answer a
conviction rather than curves it measured off the pair, so the pixel-aligned
readings matter more, not less. The head sentence carries the conviction, the
shrink, the look-error ratio against its bound *and* the re-hued share; the
admission's own foreign-hue clause follows it, the same sentence in both
places, with the same measured / not-measurable pair so a census that never
ran is never printed as `0.000`. Both fan readings are printed to one decimal:
at whole degrees a projected `7.5` — which *clears*, the test being `≤` —
rendered as "+8 degrees, inside the 7.5 degree target", and the admitted haze
pair's 14.6 rendered as "+15 … against a limit of 15".

**Ordering is the whole of the byte-identity argument.** A pair the pixel
vetoes refuse is refused unprojected, so the viaduct's recipe cannot move.
The rescue runs on exactly two calls, and both of them produce the recipe the
user gets: the `fit_cast_stage` after the mixer's do-no-harm block, and the
end-of-pipeline do-no-harm loop's re-fit, which *replaces* that recipe one
saturation step down. The mixer's do-no-harm loop judges *both* of its
branches with the cast the gates measured, because its question is about the
mixer and an invented compromise must not out-vote a per-band solve the
evidence supports: with the rescue live in every call, four fixture verdicts
moved that have nothing to do with this feature (canyon-warm's mixer flipped
from withdrawn to attached, the two-family HSL pair's the other way; that
experiment's four are a superset of the three that survive the confinement,
named below).

The second call site is exercised but its *success* is unfixtured, and that is
a measurement rather than an impression: instrumenting the loop body and
running the whole library battery logs 189 entries, nine of which re-fit a
fan-convicted cast — so the rescue is entered there with something to answer,
and in all nine it finds nothing that both clears the target and pays. No test
in the tree has ever seen a projected cast come back out of that loop, so "a
rescued cast survives the saturation step-down" is verified by reading the
code. (The comment that stood at that loop, "no current fixture reaches the
loop body", was false on this tree and is corrected in place.)

**Three consequences worth naming**, because a change that moves a calibration
owes the reader the list.

1. **A convicted cast can now land.** Where the end-of-pipeline do-no-harm
   check used to reset the whole recipe to the calibration base, the
   canyon-warm regression went from 0.0387 → 0.0387 at the 0.25 confidence
   floor to 0.0387 → 0.0339 at 0.406, its delivered sky at 216.9° against
   213.9° before, with no fan at all (that fixture's sky is one flat colour).
   Its joint-distribution reading moves with it, 0.1796 → 0.0437, which takes
   it out of the joint family's refusal band and leaves canyon-gold as that
   band's only member. The whole six-row fixture record behind that family was
   re-measured on this build and had drifted in *every* row; it now carries
   the measured numbers and their date.
2. **Canyon-warm's protection migrated, and it is measured where it belongs.**
   Because that pair's mixer now attaches first, the cast curves re-derived
   against that state rotate nothing at all — 0.0000 of the frame, against
   0.1250 with the mixer neutralised — so the rotation veto no longer fires on
   it, and what stops the violet is the composition of the mixer and the
   projection. The `ROT_DEG`/`ROT_SHARE` calibration therefore reads the gate
   on the *pair*, with the mixer neutralised, which is what that calibration
   is about; the delivered-frame guard is where the protection itself is
   checked, and it reads 216.9° against a ±30° band around 213°.
3. **The two-family HSL fixture ends where it began.** With the gain bar
   tested inside the bisection it shipped a rescue worth 0.0006 of look error,
   which took its finished residual from 0.0256 to 0.0236 — under the
   `FIT_QUANT_CLEAN` = 0.025 floor below which `unrepresented_note` says there
   is nothing left to explain — and cost that pair the sentence naming `hsl`.
   With the bar applied once, to the winner, that rescue is refused, the
   residual is back at 0.0256 and the disclosure is back with it. The
   soundness fix paid for itself here. Both halves are pinned with their
   numbers, so neither can move silently.

**Cornwall, before → after** (CLI `match`, GLOBAL stage, no zones):

| | v1.2.2 (admitted) | fan gate alone (refused) | v1.2.3 as shipped (projected) |
|---|---|---|---|
| look error | 0.137 → **0.033** | 0.137 → 0.058 | 0.137 → **0.030** |
| reported confidence | 0.646 | 0.577 | **0.664** |
| delivered sky hue spread across luma octiles | **33.1°** | 1.6° | **10.5°** |
| sky octile hues (dark → bright) | 219.2 / 226.8 / 220.1 / 193.8 | 218.9 / 219.0 / 217.3 / 217.9 | 219.7 / 220.5 / 218.5 / 210.0 |
| sky mean hue (target's is 215.3°) | 214.6° | 218.4° | 216.7° |
| cast curves | admitted as fitted | withheld, with the reading | shrunk to `t = 0.363`, +7.4° of fan |

The target's own sky reads 214.8 / 214.8 / 215.1 / 216.3, spread 1.6°.

**Why 7.5° and not 15°**, measured end to end on the same pair before the
constant was fixed:

| projection target | `t` | census fan after | look error | confidence | delivered sky spread |
|---|---|---|---|---|---|
| **7.5°** (shipped) | 0.363 | +7.4° | 0.137 → **0.030** | 0.664 | **10.5°** |
| 15° | 0.483 | +14° | 0.137 → 0.026 | 0.680 | 15.3° |

The looser target buys 0.004 of look error by delivering half again as much
fan as the visibility calibration allows. The conservative constant stands.

On this pair the shipped `t` sits below 0.5, so the three curves it emits are
*identical* — a shared luma curve with no chromatic difference left at all.
The note says so in as many words (`0.5 = one curve shared by all three
channels`), because "colour-cast curves" that carry no colour difference is
exactly the kind of thing a rationale must not leave the reader to infer. The
full sentence the shipped run writes, with the two clauses the fix-up added:

> Colour-cast curves were shrunk toward the shape all three channels share: as
> fitted they would have opened a 37.6 degree hue fan in a class holding 0.917
> of the frame's measurable colour (limit 15), so they were taken back to
> t = 0.363 of the fitted cast (1 = as fitted, 0.5 = one curve shared by all
> three channels, 0 = none). The look error with the shrunk curves is 0.525 of
> the error without them (the strength budget's bound is 2.000), and they
> re-hued 0.000 of the frame past the rotation budget. They created 0.000 of
> the frame in hues the target does not contain. The projected curves change
> that class's hue spread across luminance by +7.4 degrees, inside the 7.5
> degree target a projection has to reach.

Every number in that table is the **global** stage, measured with `match`
without `--zoned`. The showcase panel and README quote the **zoned** solve
(0.137 → 0.027 at confidence 0.65, eight masks). The global curves are
byte-identical between the two routes, so the same curves are convicted and
shrunk in the zoned run's global stage — but the zoned residual, the per-zone
dials and the final `err_after` change with them and are re-measured on the
merged build before the captions are rewritten.

The shipped fit is better than the one that carried the defect on every
number except one: the delivered sky still fans 10.5° across luminance where
the target holds 1.6°. That is the trade the projection makes, and the 7.5°
constant is where it was set. Measured against **one** population — the
target sky's own mean hue, 215.3° — the violet in the dark half comes from
11.5° off it to 5.2° (226.8° → 220.5°), and the green-cyan in the bright
cloud from 21.5° off it to 5.3° (193.8° → 210.0°). The gate's stated
tolerance applies here too: the census reads the fan the curves ADD, and the
delivered absolute spread can exceed it by up to one class width.

**The viaduct pair is byte-identical.** Its global fit was already refused by
the pixel-aligned gates, and the new note sits *between* them and "did not buy
enough" in precedence, so a pair those gates govern reports exactly what it
reported before. `cmp` on the `match` recipe JSON, before and after: identical.

That claim's scope is exact, and it is narrower than "recipes do not change".
The admission notes below **append to the rationale**, and the rationale is
part of the recipe — so a pair whose cast is *refused* (the viaduct) or never
fitted is byte-identical to v1.2.2, and a pair whose cast is **admitted** or
**projected** is not: it now carries two or three more sentences than it did,
and a projected pair's curves are different curves.

The haze regression is pinned in the TREE rather than by a probe that does not
ship (`the_haze_correction_is_never_projected`), and what it pins is the thing
that can break: the rescue arm's guard. That pair's cast reads 7.8° against a
15° line, so the fan gate never convicts it, so `earns_projection` returns
nothing and every `fit_cast_stage` call on it runs the same code with the
rescue live as without — which is *why* its recipe cannot move, and it is a
stronger statement than a one-off byte comparison against a build that no
longer exists. End to end the test also asserts the admission sentence
present, the projection sentence absent, and the curves shipped. (A literal
rescue-on / rescue-off comparison is not expressible from a test without
putting a seam in the solve, and a seam would be the more fragile pin.)

**Ordinary cast admission stops being silent.** Every way for the colour stage
to produce nothing had a note (R23-6 closed the last one), and the strength
budget disclosed when *it* bought a marginal cast — but the commonest outcome
of the whole stage, the curves shipping on their own merits, reached the user
as an unexplained presence. An admitted cast now carries the four gates' own
readings, across three notes rather than one, because two of the four can
**abstain**:

- the head note carries the look-error ratio, the bound it was judged against
  and the re-hued share — the three that are always measured;
- the foreign-hue share and the hue fan each get a *measured* clause and a
  *not-measurable* clause, so a census that never ran (a target with no
  chromatic mass; no hue class region-sized across two luma slices) says so
  in words instead of publishing `0.000` as though it had been measured;
- the ratio is stated **against its bound** rather than as "cut the look error
  to". The ratio arm rejects only when the evidence is *also* unidentifiable,
  so an admitted ratio may legitimately exceed 1.0 — and the sentence would
  then have claimed the curves cut an error they had enlarged. The bound is
  `budget.cast_ratio`, the value the path actually used (2.0 at the shipped
  default strength, up to 3.0), not a fixed constant;
- the fan is **signed**, because three channel curves can narrow a class's
  spread as easily as widen it, and "opened a −3 degree hue fan" is not a
  thing that happens.

`FIT_NOTE_CAST_ADMITTED_BY_STRENGTH` is unchanged beside all of it.

**Why nothing caught it.** The two regression fixtures for this failure family
(`warm_rock_cast_must_not_violet_the_pale_sky`,
`cast_must_not_rotate_the_sky_into_a_target_native_hue`) paint their skies as
one flat colour. A flat colour occupies one luma slice, so its curves *cannot*
sort it — the fixtures could only ever exercise the destination-shaped
question, and the pixel-aligned gates answered it. A photographed sky ramps
from zenith to horizon, and that ramp is the whole difference. The new fixture
`coast()` is the canyon geometry with exactly that one property restored.

**Verification.** Library battery 1312 passed, 0 failed, 12 ignored (set
difference against `main` @8e631f7 by test name: **+16, −0**, all in
`fit::tests` — the five the gate and its fix-up added, plus
`cast_curves_that_fan_a_coherent_sky_are_shrunk_not_shipped` (which replaces
`…must_not_fan…`), `a_projection_that_cannot_clear_the_target_leaves_the_refusal_standing`,
`the_projected_fan_grows_with_t`,
`the_bottom_of_the_projection_path_is_one_curve_then_none`,
`a_projected_cast_is_never_also_disclosed_as_an_admitted_one`,
`a_projected_cast_is_judged_by_the_strength_budgets_bound`,
`a_projection_worth_less_than_the_fits_own_quantisation_is_not_shipped`,
`a_rescored_projection_carries_its_own_readings_back`,
`the_projection_is_deterministic_and_idempotent`, and the fix-up's three:
`a_cast_the_ratio_gate_convicts_is_not_rescued_by_the_projection`,
`the_search_does_not_bisect_past_a_band_the_gain_bar_opens` and
`the_haze_correction_is_never_projected`); the `autoshade` bin battery
23; the GUI binary 160; clippy clean on default features and on
`--features gui`; `audit_i18n.py` gains no finding beyond the four pre-existing
untranslated keys and one orphan entry `main` already carries, and
`subset_gui_fonts.py --check` reports 871/871 embedded (the Chinese sentences,
including the re-hued clause added to the projection's head note, add no
glyph); `check_docs.py` 23 PASS / 0 FAIL / 5 SKIP. The Cornwall `match` runs
twice to an identical recipe, and the shipped recipe differs from the
pre-fix-up build's in the `rationale` field alone — same curves, same
confidence 0.6635828, same 0.137 → 0.030. The viaduct `match` recipe JSON
`cmp`s identical to the pre-fix artifact; the haze fixture is now pinned in
the tree rather than by a probe.

Hand mutations, each applied by an anchored byte-exact script, shown to turn a
named test red, then restored byte-for-byte (`git diff --stat` identical after
every one). From the gate: deleting the gate arm, deleting the refusal's note
arm, deleting the admission push, swapping the note precedence (which is what
holds the viaduct byte-identical), dropping the fan's baseline subtraction,
collapsing an abstaining reading back to `0.0`, hard-coding the ratio bound to
the default anchor. From the projection: disabling the projection arm,
relaxing its target from 7.5° to the 15° refusal line, freezing the path's
lower leg (so `t = 0` is the shared shape rather than the identity), freezing
its upper leg (so the chromatic part is never restored), dropping the
`FIT_QUANT` gain bar, dropping the admitted/projected exclusivity, deleting
the "the shrink was tried" clause from the refusal sentence, and dropping the
projection's readings on the rescore path. From the fix-up: dropping the ratio
gate from the rescue's precedence; putting the gain bar back inside the
bisection (which turns *two* tests red — the soundness pin, and the
two-family residual, which falls to 0.02356); disabling the identity→empty
rule; halving the fan gate's conviction line so the haze correction reaches
the rescue arm; dropping the re-hued arg and dropping the foreign clause from
the projection's notes; taking the admitted fan clause back to whole degrees;
relaxing the projection target to 15° with the gain bar off (the delivered sky
then fans **32.6°**, against the 15° bar the coast test carries); raising the
gain bar tenfold; and cutting the bisection from 12 steps to 4, which leaves
canyon-warm landing — but at 0.03513 instead of 0.0339, which only the number
can see.

One guard is deliberately unfalsifiable and is reported as such: the
`cast.projected.is_none()` test that keeps `FIT_NOTE_CAST_ADMITTED_BY_STRENGTH`
off a projected cast. Removing it leaves the whole 1312-test battery green,
because the projection's gain bar forces a rescued candidate's ratio under 1.0
while that sentence needs it above 2.0. It ships anyway, with a why-comment
saying so: the doctrine is one head note per outcome, not one head note per
outcome that happens to be unreachable.

## The Style axis and a direction: who leads

Write a Direction and the AI now follows it, even when it takes the photograph
somewhere your own past edits have never been. Until this release your style
library always won that argument.

### The defect

On the island showcase frame, at `--style 1.0 --strength 0.9`, against this
photographer's own index (169 RAW+XMP exemplars plus 94 finished looks), three
directions as far apart as *dark moody low-key … teal-and-orange*, *warm golden
tones, film-like grain, lifted matte shadows* and *vivid saturated colours,
punchy high contrast, crisp clarity* came back nearly alike — all three inside
the library's own cool hazy register:

| Panel cell, mean HSV S/V (% of 255) | neutral | moody | golden | vivid |
|---|---:|---:|---:|---:|
| v1.2.2, the photographer's full index | 17/47 | 23/54 | 11/58 | 17/55 |
| The same three directions, own edits removed from the index | 17/47 | 34/38 | 12/61 | 29/65 |

The second row is what the directions can do when nothing holds them back. The
first row is what the app shipped: across the three, the brightness spread was
**4 points**.

### Root cause

One mechanism with two halves, and neither had a third answer.

`StyleIndex::render_reference` switched on a single boolean, `style >= 0.85`, so
the reference block could say exactly two things: below that band the retrieved
habits were a CEILING ("stay within it, do not exceed it"), at or above it a
TARGET ("REPRODUCE this look"). Either way the library was the thing to hit, and
the direction could only move the proposal *within* it.

Then `pipeline::produce_recipe` lerped the finished proposal toward the
neighbours' arithmetic means with `style_pull` — which at Style 1.0 is FULL, i.e.
the proposal's own value is replaced. A direction that had won the argument in
words lost it in arithmetic, one function later.

### The fix

`style::StyleVoice::choose(style, direction, adherence)` is now the one place
that decides, and it decides both halves together:

* **Wording.** A third voice, `Background`, is chosen when a non-empty Direction
  meets adherence tier **Direct** or **Brief**. Its header reads *"STYLE
  BACKGROUND — how this user edited SIMILAR past shots, for continuity only …
  The DIRECTION LEADS: where it and these habits conflict, follow the
  direction"*, and each of the four aim clauses — tone curve, colour families,
  shared look, local work — becomes a habit the direction may override.
* **Arithmetic.** `pipeline::style_blend_pull` returns nothing in that voice, so
  neither the verified proposal nor a visual-judge candidate is pulled toward
  the library's means, and the re-verification that follows a real blend is
  skipped with it.
* **The reviewer's brief.** The visual judge is briefed in the same voice. With
  the direction leading, the shared look of your own past edits is stated to it
  as *continuity, not the brief* — something it must neither enforce nor
  penalise — and the standing refusal to buy a revision that walks a look back
  is re-aimed at the DIRECTION. This is not cosmetic: the judge BUYS revisions,
  and two of the three acceptance develops adopted one, so the reviewer that
  chose the recipe that shipped was the last place still calling the library
  the brief. Below tier Direct, and with no direction at all, its rubric is
  byte-identical to v1.2.2.

The control is the **Adherence** dial the app already had — the dial whose whole
job is "how strongly should this direction be followed" — not a fourth slider.
Drop Adherence to 40 % or below (tier *Hint*) and the library leads again.

That dial now reaches every surface, which it did not before. `--adherence` on
the CLI and the Adherence slider in the desktop app were already there; the
browser page (`autoshade serve`) gains an Adherence slider beside Style
influence, and `POST /api/analyze` gains an optional `adherence` field (0..1).
Omitted, it is the same 0.65 every other surface defaults to, so a client
written before this release sends exactly the request it always sent. Without
it, every web develop carrying a direction would have been forced into the new
voice with no way back — the one surface that could not choose.

### What it does to the showcase frame

The same three directions, the same 169+94 index, the same dials, on the
v1.2.3 release build (the panel shipped in the showcase):

| mean HSV S/V (% of 255) | neutral | moody | golden | vivid | brightness spread |
|---|---:|---:|---:|---:|---:|
| v1.2.2 | 17/47 | 23/54 | 11/58 | 17/55 | 4 |
| **v1.2.3** | 17/47 | **28/43** | **11/58** | **30/70** | **27** |
| own edits removed from the index (the ceiling on what a direction can do) | 17/47 | 34/38 | 12/61 | 29/65 | 27 |

`vivid` lands within one point of the no-library run's saturation and five
points brighter than it; `golden` matches its saturation and sits three points
darker; `moody` recovers the brightness collapse that made all three read alike
— 43 against v1.2.2's 54, five points above the no-library run's 38 — at six
points less saturation. Summed over the six numbers, the distance from the
direction-led develops to the no-library run falls from 53 to 21, and the
brightness spread across the three is 27 points, the no-library run's own. The
neutral conversion is the control and reads 17/47 in all three panels. The
branch's own triple on the pre-bump merge build, independently re-measured by
the reviewer, read 26/43 · 11/60 · 29/63 (spread 20, distance 17): the advisor
is not deterministic, and the two runs tell the same story.

### What does not change

Retrieval is untouched: the Style slider still gates it, the direction still
ranks your exemplars and your look library, the reference photo still rides
along as IMAGE 2, and `STYLE_NEIGHBOURS` / `STYLE_REF_IMAGE` / the look notes
still name what answered — the acceptance develops each disclose the same four
neighbours they always did. The measured NUMBERS printed in the block are
byte-identical in all three voices: a dial must never restate what the
photographer actually did.

The look-library block still says *match its grade, not its content*, and the
reference photo still arrives as IMAGE 2 with *match that LEVEL of grading*.
That is deliberate and it is not a contradiction: the finished photo in both
places is ranked WITH your direction among the query terms, so following its
grade is following the direction. The half that is not direction-ranked — the
shared tags of your own past RAW+XMP edits — is the half the new voice demotes,
in the block and in the judge's brief alike.

With **no** Direction, a blank one, or one at tier *Hint*, the block the model
receives is byte-identical to v1.2.2 at every Style value.

A develop that skipped the pull says so, on every surface:

> *your style library was kept as BACKGROUND for this develop — the direction
> leads at adherence tier direct, so no style-distillation pull was applied to
> these numbers; lower the Adherence dial to 40% or below to hand the library
> back the lead*

It names the dial, not a flag: this note is persisted into the recipe and
re-rendered in the desktop app and the browser, neither of which has a command
line to type one on.

### Why nothing caught it

Every existing test asked whether the block said the *right* thing for a Style
value, and it always did. There was no test — and no code path — for the
question the ruling asks, because the product had no answer to it: the two
voices were exhaustive by construction, and the blend was gated only on "was
anything retrieved?". Nothing in the battery could distinguish "the library is
the target" from "the library is all there is".

The judge's brief survived the first cut of this fix for the same reason one
step further out: it lives in a different file, keyed off a different value,
and every test it had asked whether the look reached the reviewer — never whose
aim it was. And the two blend call sites survived because no test in the
battery can reach them: the blend runs only inside a paid analysis, so an
unguarded pull compiles and stays green.

### Verification

* Byte-identity of the two shipped voices is pinned against three fixtures
  captured from the v1.2.2 build before the third voice existed, covering both
  arms of the colour sentence, through both entry points, for no direction /
  blank direction / Hint direction.
* `style-query` grew `--adherence` and now prints the voice a develop would
  send, so the free diagnostic cannot disagree with the paid run.
* CLI `--style` / `--adherence` help and the GUI Style / Adherence tooltips (en
  and zh) say who leads.
* The widest BACKGROUND block measures 3,865 B against the proposer's 4,096 B
  reference budget; the widest block overall is still the Target
  dial-allowance arm at 3,969 B.
* The judge's brief is pinned the same way: the `Ceiling` and `Target` rubrics
  are compared byte-for-byte against a fixture captured from the build that
  predates the change.
* Both blend call sites — the verified proposal and the judge candidate — are
  held by a source guard that counts them against the voice checks that gate
  them, because no test can run the blend path itself without a paid analysis.
  Restoring an unguarded pull at either site used to leave the whole battery
  green.
* The web body's own end is tested: a request carrying `adherence: 0.3` with a
  direction keeps the historical voice, and one that omits the field lands on
  the shipped default; a hostile number goes through the same clamp the other
  two dials use.

### What these numbers do not carry

* **The crop.** One of the three shipped v1.2.3 develops cropped — `vivid`,
  9504×5702, 7 % off the top and 3 % off the bottom — where `moody`, `golden`
  and the v1.2.2 trio rendered the full 9504×6336 frame (the pre-bump triple
  cropped all three). Nothing in this change touches cropping and the proposer
  has always been free to crop, but that cell samples a slightly different
  framing, so its v1.2.2 → v1.2.3 delta carries an unquantified framing
  component. The neutral control cell is bit-identical across all three panels
  (16.80 / 46.86 before rounding), which is what makes the comparison worth
  reading at all.
* **n = 1.** One frame, three directions, one run each. The advisor is not
  deterministic; a second triple does not reproduce these digits — the two
  triples above differ by up to seven points in one cell. The separation claim
  rests on two paid triples that agree in direction, one of them independently
  re-measured.
* **"Byte-identical at every Style value"** is pinned by measurement at Style
  0.30 and 0.90 and at the 0.85 split boundary, and by construction elsewhere:
  the only other continuous read of the Style value inside the block is the
  Target dial-allowance arm, which this change does not touch.

## Measured, and kept absolute — the luminance-range boundary budget

The v1.2.2 seam ruling named a second finding it did not act on: the
luminance-range family keeps its own absolute `RANGE_BOUNDARY_RIM_MAX = 0.012`,
and range bands live in luminance transitions — by construction the smooth
gradients where a ramp is most visible. Nobody had measured whether that scalar
hides the defect the tile budget did. It does not, and this release records why
in numbers rather than in an argument.

Driven first-party with segmentation and correspondence made unavailable, so
`match --zoned` takes the luminance-range fallback. A band attaches on the
calibration pair (luma `[0.471, 0.765]`, −0.56 EV) and on the stone-viaduct pair
(luma `[0.118, 0.882]`, −0.80 EV and saturation +23); the Cornwall pair attaches
none.

Two rasters are in play, and the table keeps them apart. The **engine** gates on
the 384-px thumbnail it develops itself, and that is what the rationale
discloses. The readings under it are a transcription of the same statistic over
a full-resolution develop downsampled to a 384-px long edge, taken on the basis
the engine passes its ruler — the globals-only **twin**, the frame with the
global correction and no range masks.

| | calibration | stone viaduct |
|---|---:|---:|
| Gate verdict, engine raster | `k=1.000`, 18 651 crossings | `k=1.000`, 1 214 crossings |
| Transcription raster, twin basis | 18 297 crossings | 1 224 crossings |
| Same ruler on the **uncorrected** render | p90 0.00392 (1.00 code), max 0.00978 | p90 0.00874 (2.23 codes), max 0.00978 |
| Same ruler on the **delivered** render | **p90 0.00230 (0.59)**, max 0.01217 | **p90 0.00857 (2.19)**, max 0.01407 |
| Delivered transfer over the transition | 0 reversals / 30 bins, min slope +0.019, max +0.964 | 0 reversals / 19 bins, min slope +0.855 |
| Mask-free `rim_overshoot.py`, 1200 px | mean 0.0006, p90 0.0018, max 0.0082 | not applicable (below) |

The correction moved the gate's own ranked reading **down on both pairs**, and
the mask-free ruler — whose control against an identical render is exactly
0.0000 — reads 0.46 of a code at the p90.

### Why the scalar is right here and was wrong for the tiles

The two rulers measure different things. `boundary_step` differences the scene
away, so a tile seam in clean sky and one in texture arrived at the same flat
budget; that is the defect v1.2.2 fixed with a per-crossing charge.
`range_transition_rim` never differences the scene away: it admits a neighbouring
pair only where the *reference* crossing is already smooth (`|Δl| ≤ 2.5/255`) and
then reports the **rendered** gradient there. The scene's own gradient is inside
the reading and is spent first. That argument was already in the tree — the
comment inside `range_transition_rim` has said since v1.2.2 that a graded context
here is capped near 2.5/255 by construction and so has no dynamic range. What is
new is the measurement behind it and a test.

Charging this ruler the way the step ruler is charged would **over-tighten** it,
not loosen it. The v1.2.2 charge is `raw × (MAX ÷ budget)` with `budget` clamped
at or below `MAX`, so the multiplier is ≥ 1 by construction and can never weaken
a gate; and here the context term is the admitted crossing itself, capped at
2.5/255 = 0.0098 and therefore always under the 0.012 ceiling. The pass condition
would collapse to "scene + correction ≤ scene" — no steepening admitted anywhere,
which refuses nearly every band.

What the budget buys is a **p90**, not a per-crossing cap: the reading is the
90th percentile of |signed bow|, so a tenth of the crossings rank above it and
are not bounded at all. At that rank, the widest crossing the window admitted on
either pair measures 0.00978 luma (2.49 codes) against the budget's 3.06, so a
correction adds 0.57 of a code where the scene already fills the window and the
whole 3.06 only where it is flat — 1.22× the steepest crossing the window will
call smooth. The measured maxima sit above both, as a p90 gate permits: 0.00978 →
0.01217 on the calibration pair (1.24×) and → 0.01407 on the viaduct (1.44×, i.e.
1.09 codes added at that one crossing, past the 0.012 budget itself).

A tile edge is an arbitrary rectangle laid across continuous sky, so any step
there is an artefact. A range edge *is* an iso-luminance contour of the
photograph, so a difference across it is the correction doing its job, and only
the transition's shape tells the two apart. Measured, the shape is a ramp.

### Why the seam statistic does not decide this one

Stepping across the calibration band's contours in 8-bit codes at 1200 px — the
shape that decided the tile seam — gives four measured rows, and they disagree
with each other by a factor of five at the same contour:

| contour | basis | introduced | control sd | z |
|---|---|---:|---:|---:|
| `lo_outer` | twin | +9.05 codes | 0.853 | +10.08 |
| `lo_outer` | neutral | +2.83 codes | 1.301 | +1.94 |
| `lo` | twin | +7.18 codes | 0.952 | +6.99 |
| `lo` | neutral | +8.28 codes | 0.873 | +8.92 |

At the `lo_outer` contour the p90 |·| is 22.84 codes, 26.8 times the control's
own spread. That is the reading a sceptic would call the defect, so it goes on
the record rather than being left out. It is not diagnostic here, and the swing
between two honest bases is why: a range edge IS an iso-luminance contour, so
this statistic measures the correction rather than an artefact. The transfer
table settles the direction — the ramp **compresses** (maximum slope +0.964
inside it, 30 input codes into 9.9 delivered ones, 9 of the 30 bins under slope
0.1), so those 9 codes are the scene's own gradient being flattened, not a step
being introduced.

### What nothing caught, and still does not

Two limits go on the record with the table.

The mask-free ruler is **not applicable** at the viaduct's contour, by its own
numbers: its 60 px plateau windows must bracket the transition, and there the
band's own spread (40.4 codes) exceeds the plateau gap (22.1) on 201 of 231
columns.

And this ruler ranks **magnitude**, so it cannot tell a preserved gradient from
an inverted one of the same size — and neither can `rim_overshoot.py`, because
the *difference* it ranks stays monotone even where the delivered luma does not.
Measured on a synthetic 16-bit grey ramp, at ramp widths this producer emits.
Each cell is the minimum delivered transfer slope, or where negative, the depth
of the non-monotone excursion in codes:

| ramp | −0.35 EV | −0.56 EV | −0.80 EV | −1.10 EV | −1.50 EV |
|---|---|---|---|---|---|
| 1.5/17 | +0.000 | reversal, 2 codes | 8 | 16 | 26.5 |
| 1/17 (`RANGE_MIN_RAMP`) | reversal, 1 code | 7 | 14 | 22.5 | 32 |

`rim_overshoot.py` reads max 0.0000 over its full n = 1024 on every one of those
rows. The widest ramp the producer emits, `RANGE_MAX_RAMP` = 2/17, is **absent
from the table on purpose**: that instrument needs 180 px of margin each side and
the probe's locator lands exactly on row 180 of its 512-row frame, so every
column is rejected and it returns n = 0. Those rows are unmeasurable by this
probe, not measured clean. The probe also pins the band at the calibration
position, and luminance POSITION is a third axis it does not sweep — the
viaduct's real 2/17 band at −0.80 EV sits at codes 0–30 and does not reverse.

Neither real pair reaches that corner; the calibration band's own minimum slope
is +0.019, which is one unit of the transfer estimator's own noise floor
(±0.05, measured over the zero-weight control region where the true slope is
exactly 1.000) away from a reversal. The answer is a sign test on the delivered
transfer rather than a re-tune of this ceiling, so it is registered for its own
batch instead of being folded in here.

## The Chinese UI fell back to English for four rationale notes

v1.2.2's verification claimed `scripts/audit_i18n.py` reported 0 findings. That
was not true of the tagged tree: run on `v1.2.2` the audit exits 1, with four
rationale sentences carrying no Chinese entry and one orphaned Chinese entry
matching no call site. A reader on the Chinese UI got those four notes in
English, mid-paragraph, inside otherwise translated rationale.

The four are the zoned boundary-continuity gate's "kept" and "inert-drop" notes,
the "no zoned correction attached — everything solved to neutral" exit, and the
note saying which frame a residual was measured on. The orphan is the same
"kept" note's old translation: v1.2.2 reworded the English from *signed
transition rim* to *introduced transition rim* and the Chinese entry stayed
keyed to the old wording, so it stopped matching and the sentence fell through
to English. v1.2.3 translates all four, retires the orphan, and the audit now
exits 0 with every section at 0. No new glyph was needed — the wording was
chosen from the hanzi already in the embedded subset, so
`scripts/subset_gui_fonts.py --check` stays green without regenerating a font.

### Verification

No fitting code changed for the boundary item, so every delivered render is
byte-identical by construction: the diff is rustdoc, one markdown section, one
test, and the GUI translation table. One test was previously absent and now pins
the scale the argument rests on — the smooth-crossing window bounds an untouched
render's own reading (exactly 2/255 on the fixture), the budget clears that
reading by the measured margin, and this family declines the contextual charge.
Three hand mutations turn it red and were reverted byte for byte: budget
0.012 → 0.007, window 2.5/255 → 4.5/255, and `charged = rim × 1.5`.

One gap in the release battery closed with this release: until now
`AUTOSHADE_FIT_CALIBRATION_DIR` was unset on the build machine, so every
calibration-corpus test in the tree returned without running. The final
battery of this release runs one lane with it set (see the table under
"What else is in v1.2.3"), and that lane is part of the release battery from
here on.

## What else is in v1.2.3

- **The showcase is re-rendered on this build.** The four-looks panel is now
  the run against the photographer's full index — 169 RAW+XMP exemplars and 94
  finished looks — at `--style 1.0 --strength 0.9`, the configuration v1.2.2
  had to step away from; the finished-look-only run stays on the showcase page
  as the contrast. Measured on the panel's cells, mean saturation reads
  28 % / 11 % / 30 % for moody / golden / vivid against the conversion's 17 %
  and mean brightness 43 % / 58 % / 70 % against 47 %, where v1.2.2's panel on
  the same index read 23 / 11 / 17 and 54 / 58 / 55 — the brightness spread
  across the three directions goes from 4 points to 27. The Cornwall reverse fit is re-run on the
  same build with its zoned solve: look error 0.137 → 0.027 at confidence
  0.66 across two semantic zones, four boundary-gated tiles and two field
  masks, and the delivered sky's hue now spreads 9.6° across its octiles where
  v1.2.2's admitted cast alone fanned the same sky 33.1°. The stone-viaduct
  panel is unchanged: its cast is refused by the pixel vetoes before the new
  gate or the projection is consulted, and its `match` recipe is
  byte-identical to v1.2.2's.
- **`autoshade serve` and `POST /api/analyze` take `adherence`.** Optional,
  0..1, same default as the CLI and the desktop app; a matching slider sits
  beside Style influence on the embedded page. `style-query` grew
  `--adherence` and prints the voice a develop would send.
- **Ordinary cast admission is disclosed.** A reverse fit whose cast curves
  ship now says so in the rationale, with the readings that admitted them —
  the look-error ratio against its bound, the re-hued share, the foreign-hue
  share and the signed hue-fan change — and a reading the census could not
  take is written as not measurable instead of as a zero.
- **The calibration-corpus tests run.** `AUTOSHADE_FIT_CALIBRATION_DIR` is set
  for one lane of the release battery, so the tests that pin the calibration
  pair's numbers execute instead of returning early. That lane reports the
  same counts as the default one — 1320 passed, 12 ignored — because those
  tests are ordinary `#[test]`s that returned early without the directory
  rather than `#[ignore]`d ones; what changes is that they now measure.

## Upgrading

Install over v1.2.2; the installer upgrades in place. No schema, sidecar or
store format changes, and a browser client written against v1.2.2 sends
exactly the request it always sent. Three behaviours change on purpose: a
reverse fit whose fitted cast would fan one hue class across luminance is now
projected toward a shared curve shape, or refused when even that cannot clear
the limit, and says which; every reverse fit whose cast is admitted carries the
admission's readings in its rationale, so those recipes are no longer
byte-identical to v1.2.2's; and an AI develop given a Direction at Adherence
above 40 % is no longer pulled toward your style library's means. Everything
else renders byte-identically.

## Verification

Release battery on the tagged tree, in parallel lanes with their own target
and data directories:

| Lane | Result |
|---|---|
| Library (`cargo test --release --lib`) | 1320 passed, 0 failed, 12 `#[ignore]`d forensic probes (1332 enumerated) |
| Library again with `AUTOSHADE_FIT_CALIBRATION_DIR` pointing at the calibration corpus | 1320 passed, 0 failed, 12 ignored — the calibration-corpus tests now execute instead of returning early |
| CLI binary | 23 passed |
| Contract tests (`tests/repro_*`) | 2 + 2 passed |
| Desktop GUI (`--features gui --bin autoshade-gui`) | 160 passed |
| `cargo clippy --all-targets`, default and `gui` | 0 warnings on both |
| `scripts/check_docs.py --gates <transcript>` | 28 claims: 27 pass, 0 fail, 1 skip (the XMP census, whose corpus is outside the repo) |
| `scripts/audit_i18n.py` / `scripts/subset_gui_fonts.py --check` | every section 0 (the four notes v1.2.2 shipped untranslated are now translated and the orphan retired) / 871 of 871 glyphs embedded |

Set difference of library test names against v1.2.2's tree (`8e631f7`):
**+24 / −0** — sixteen in `fit::tests` for the hue-fan gate and the projection
(the gate's five, the projection's eight, the fix-up's three, one of which
replaces `cast_curves_must_not_fan_a_coherent_sky_across_luminance` by name),
the range-rim budget window test, and seven for the direction-led Style voice
(`StyleVoice`, the judge's brief, the blend-site source guard, the web
`adherence` field, the no-direction byte-identity pin). The GUI battery is
unchanged at 160.

Hand mutations run and reverted byte for byte, each turning a named test red:
twenty-eight on the cast stage across its three rounds (gate, fix-up,
projection — the list is in the projection section above), three on the range
budget (0.012 → 0.007, window 2.5/255 → 4.5/255, `charged = rim × 1.5`), and
eight on the Style voice (the Background arm of the reference block, the
judge's Background rubric, one of the two blend guards, the web field's
default, and four on the byte-identity fixtures). One guard is deliberately
unfalsifiable and is named as such in the projection section.

Every number in this document was reproduced by an adversarial reviewer from
a build of its own, and the three items were each reviewed twice: once on the
first implementation and once on the fix-up its findings produced. The cast
projection went through a third round after its second review, on the
soundness of the search and on sentences that had drifted from the code.

Cornwall, zoned solve on the tagged tree: look error 0.137 → 0.027 at
confidence 0.66; the recipe differs from the pre-release build's run only in
the reworded rationale, and the rendered panel is byte-identical to it.

All six release assets were downloaded from the GitHub release (run
33641516217: windows, macos, macos-battery and publish all green) and verified
with `sha256sum -c checksums.txt` and an independently computed SHA-256 before
the README and site tables were filled from those bytes; the downloaded CLI
reports `autoshade 1.2.3`. The local install was upgraded in place from the
downloaded installer and both executables compared byte for byte with the
release checksums (registry 1.2.3, the 19 weight files untouched). autoshade.dev
was deployed from `site/` and byte-verified against it, Cloudflare's beacon
stripped: 22 files identical, 0 mismatched.
