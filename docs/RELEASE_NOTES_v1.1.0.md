# Autoshop v1.1.0 — layered reverse fit, per-band colour, style retrieval with eyes

> **Erratum (added 2026-09-02).** This release also changed the library API
> without saying so: `generative::reimagine` returns `Result<ReimagineReport>`
> (the frame divergence and the first divergent step) instead of `Result<()>`,
> and the image call returns a typed `GeneratedImage` instead of a
> `(Vec<u8>, String)` pair. The CLI and GUI were updated in the same commit;
> anything else that links the crate had to follow.

Autoshop v1.1.0 is the largest release since 1.0: the reverse fit learns to
work in layers (semantic regions, luminance ranges, spatial tiles and a
free-form remainder), gains a per-band colour mixer, and puts the whole
honesty budget on the Strength dial. Style retrieval grows three optional
similarity rulers and a look library, and the generative path survives relay
transports and subscription size caps. Two deliberate render hard changes are
called out below; everything else at default settings is byte-identical to
v1.0.0 unless a section says otherwise.

## Render hard changes

- **Linear-gradient falloff is now eased.** v1.1 ships a measured C1 Hermite
  smoothstep falloff for linear gradient masks, giving Lightroom-like soft
  handles while preserving the existing handle transport and XMP schema
  (`817fa13`). Old recipes containing linear masks re-render with a softer
  transition between the handles; the handles themselves and both plateaus are
  byte-identical, and radial and bitmap masks remain byte-identical to v1.0.0
  (each pinned by a named test). The basis: a hand-made Lightroom gradient
  probe shows 80/80 rows of curvature at the ends versus 0/3 for the old
  clamped ramp, and the free-endpoint fit prefers smoothstep at rms 0.0045
  over linear at 0.0169.
- **The reverse fit writes the per-band colour mixer (stage 4a).** At default
  strength the reverse fit now solves `hsl.saturation` and `hsl.luminance` one
  ACR band at a time from that band's own population statistics (`hsl.hue`
  stays 0 always), so the same (source, target) pair yields a different recipe
  and a different render than v1.0.x (`ab01520`). The persisted schema is
  unchanged — `hsl` is an existing field, `recipe.json` and XMP round-trips
  are unaffected, and reading an old recipe behaves exactly as before.
  Measured: p36 finished-frame error 0.032592 → 0.031792 (confidence
  0.6657 → 0.6752); the stage is judged twice, once where it is fitted and
  once after the cast curves against its own absence on the finished frame.
  The persisted rationale text changes: the summary strings narrow to "local
  masks and per-band hue rotation are not recovered".

## The reverse fit works in layers

- Reverse fitting now runs `global -> (semantic regions OR luminance ranges)
  -> spatial tiles -> free-form remainder masks`, always global-first, over
  frozen original-pair evidence with a depth-2 quadtree, a four-tile cap,
  re-derivation after every attachment, and a zero tile frame-regression
  tolerance (`67084b2`, `be85702`, `662b688`). Semantic and luminance layers
  are deliberately exclusive: when semantic segmentation succeeds the
  sky/ground bitmaps are kept, and only when it is disabled or unavailable are
  luminance ranges tried; with no acceptable segment the global result stands.
- **Up to four semantic regions, opt-in.** `match --zoned --regions 2..4` and
  the GUI "Up to four semantic regions" checkbox open disjoint ADE20K-class
  regions, each independently choosing Full or Atmosphere mode, with overall
  confidence taken from the worst accepted region (`32b0fe4`). The four-region
  trial and the seeded two-region result are arbitrated on the same evidence
  ruler; ties fall back to two regions with a typed `REGION_FRAME_REFUSED`
  disclosure. The default remains the historical sky/ground routing,
  byte-identical to v1.0.x in dials and confidence — the only default-visible
  change is a typed `ZONE_ALREADY_MATCHED` rationale sentence for regions
  that need no correction.
- **Structural divergence chooses the mode.** Regions whose content genuinely
  differs between source and target (generated clouds, replaced skies) are
  fitted in a bounded Atmosphere mode instead of the Full estimator, and
  divergent regions are never silently dropped (`5aaeea4`, `10e02bb`).
- **Luminance ranges export natively.** Native luminance-range partitions are
  carried by a full-frame LINEAR sentinel over the observed domain and written
  into Lightroom XMP as an intersect range; bitmap semantic zones remain
  engine-only, rendered losslessly from recipe JSON and omitted from classic
  XMP with the existing named bitmap loss — no four-gradient approximation is
  implied. Every luminance segment's attach / abstain / merge decision,
  boundary rim, shared shrink `k` and typed refusal is disclosed per segment;
  one-sided or zero structural evidence is never silently read as "equal".
- **Refinement is conservative.** Gate-guided mask refinement distinguishes
  kept from abstained semantic/tile refinements, reruns the normal rim and
  frame gates after refinement, and never refines luminance ranges.
- **The spatial-tile boundary gate now measures a real seam.** v1.0.x's
  boundary-continuity gate read only soft transition pixels, which a spatial
  tile's hard 0/255 mask does not have, so the gate passed vacuously — the
  visible rectangular seam in a fitted sky was never caught. v1.1 measures the
  cross-boundary step (a difference-in-differences along the mask contour)
  differenced against the render without the correction so a subject edge
  under the border cannot false-positive; zero measurable crossings is now a
  refusal, not a pass; feathered semantic masks keep the measured, non-vacuous
  transition-band ruler. On the shipped viaduct frame the seam tile now reads
  `cross-boundary step 0.0350 -> 0.0118` (k=0.372, 160 crossings, budget
  0.012) where it read `0.000 (0 transitions)`, the seam falls p90
  0.0250 -> 0.0052 on the mask-free ruler, and the fit honestly pays look
  error 0.015 -> 0.017 — inside its global-only ceiling of 0.019.
- **Strength governs the honesty budget (the freedom axis).** `match
  --strength` / the GUI Strength slider now scales the reverse fit's evidence
  budgets end to end (`302efb1`). The 0.65 default is byte-identical to v1.0.x
  except that a directionally consistent global colour cast is now measured at
  every stop and disclosed as `FIT_NOTE_GLOBAL_CAST`; above the default, WB
  shrinks along the manifold with typed disclosure, and at strength >= 0.85
  vetoes become disclosures with the confidence capped (0.414 at 0.85, 0.35 at
  1.0). At Style >= 0.85 the reference block adopts "TARGET style" wording.

## Style retrieval and the advisor

- **Three optional similarity rulers** (`74a1e93`): SigLIP 2 image embeddings
  (`--embed` / GUI "Use image embeddings", default off), Direction text
  scored against candidate images, and local descriptions compared to the
  request (`--describe`, requires `--embed`; Qwen3-VL-2B local sidecar, first
  run downloads 4.3 GB of weights, CUDA bf16 uses about 4 GiB of VRAM). With
  every switch off, retrieval, the reference block and the recipe are
  byte-identical to v1.0.x, pinned by a named test. A finished-look library
  (`looks`, up to 500 JPEGs) feeds only the prompt and reference image, never
  the fit targets. Description caching lives in the user store, keyed by frame
  bytes, model and prompt version, capped at 20,000 entries.
- **The text term is hubness-corrected and re-weighted** (`13c262e`): some
  exemplars scored high against every direction word, so each candidate's mean
  stored vocab cosine is subtracted before the z-score, and `W_TXT` moves from
  4 to 0.5 — MAE 0.688864 against baseline 0.713143, CI
  [+0.005837, +0.041111]; opposite directions now share a top-1 only 44.7% of
  the time (previously 71%), and 149 of 169 exemplars are actually retrievable
  (previously 52). `AUTOSHOP_STYLE_TEXT_WEIGHT` keeps its name with the new
  semantics; the zero-weight path stays bit-for-bit.
- **The judge sees the style it grades** (`f03b08a`): grading intent carries
  the retrieved look's own summary phrases, from the same source as the
  reference block, so the judge and the proposer never describe two different
  looks. The colour-habit floor is never an empty claim: below the measured
  floor the advisor cites the strictly positive per-dial permission instead.
- **Mask habits widen — a forward-incompatible index.** The style index's
  per-mask habit vector grows from 8 to 10 sliders (in-mask temperature and
  tint) plus `curved`, the share of uses carrying a local point curve. A v5
  index written by this build is refused by older builds with
  `invalid length 10` and must be rebuilt there; this build still reads the
  old 8-wide shape with missing columns zero-filled.
- **The reference photo can ride along**: `--reference-image` on `analyze` /
  `auto` (or `AUTOSHOP_SEND_REFERENCE_IMAGE=1`) also sends the retrieved
  reference photograph itself with the proposal. It is Destination-trust: only
  your own environment or user-level settings can enable it, a downloaded
  photo pack's `.env` cannot, and `batch` never sends one. Default off.

## Generative path

- Whole-frame generation carries an unconditional fidelity preamble, measures
  generation-side structural divergence with the same statistic as the fit,
  and offers an opt-in bounded fidelity retry (`0399c88`).
- **The OAuth (Codex bridge) image mode actually completes now** (`a6e5a03`):
  the bridge labels JSON bodies as SSE, so v1.1 sniffs the body instead of
  believing the header, and a subscription tier's same-aspect size cap (long
  edge >= 1024, aspect within 0.5%) is accepted rather than refused, with the
  delivered size disclosed in both the terminal and the GUI. Previously the
  published OAuth image switch could not finish a Reimagine at all.

## Application

- Cross-image correspondence (DIFT / SD 2.1) joined the sidecar family and
  feeds the estimators behind the divergence gate (`d176205`, `8319e6c`); the
  tone estimator uses paired robust regression over corresponding pixels
  (`94cc7aa`).
- The GUI holds a process-wide memory and thread budget (`4b7362f`), variant
  switching no longer reads as unsaved (`3eee65c`), and typed refusal
  disclosure carries through every new layer (evidence classes, per-range
  measurability, sanitized sidecar errors).
- **Releases are now built by GitHub Actions** (`187f361`): every asset below
  is reproducible from this tag on a public runner — the reproducibility
  promise made on issue #2 after a locally built exe drew a Defender false
  positive. `checksums.txt` in the release carries the SHA-256 of each file.

## Assets

The asset table (name, size, SHA-256) is generated by the release workflow and
published with the release; verify a download against `checksums.txt`.
