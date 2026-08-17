# Autoshop — Architecture

> Status: **implemented** (v0.27.0 — R21: deleted version snapshots STAY
> deleted. One root cause, three resurrection arms: the backup gate's only
> "already preserved" dedup witness was the deletable snapshot itself,
> `claim_version` re-issued a freed number (max+1) so the reborn snapshot
> wore the deleted label, and the legacy `./out` migration re-published a
> deleted `v<N>` from the retained legacy file on every open. One
> mechanism closes all three: a delete registers its number + content
> fingerprints (raw bytes / structure + raster bytes / xmp) in the
> develop's permanent `.deleted-versions.json` — numbers are never
> re-issued, the gate stops AUTO-preserving explicitly discarded content
> (a discard record, not a recovery copy; explicit 「＋ Save as version」
> stays ungated), and the migration skips burned numbers (§4.4/store).
> v0.26.2 — R20: the VISUAL JUDGE role closes the
> first pixel-level loop on AI output (`advisor::judge`, §3): interactive
> analyze renders the verified proposal and has the vision model score it
> — a low score buys ONE guided revision, adopted only if it re-judges at
> least as high (do-no-harm; batch/eval skip the paid loop); the
> reverse-fit gains an opt-in AI review (GUI checkbox / `match
> --ai-judge`) scoring target-vs-fitted match 0-100; the judge sits at
> the END of the look-mutation chain (post style-distillation, WB anchor
> from the one hoisted calibration snapshot, refine masks carried into
> the render clone) so the judged render IS the delivered look; the
> histogram evidence now carries luma quantiles + per-channel means.
> v0.26.1 — R19: every remaining recorded item
> closed — the zone skip line and acceptance floor are split so nothing
> fixable is declined untried, the misprediction gate's anchors are
> measured in BOTH solve domains for all five real pairs, and the
> Generate button's row math is exact (measured label + owned margins,
> one-line by fixture). v0.26.0 — R17+R18: the reverse-fit's tone
> evidence polices its own identification (a misprediction gate falls the
> solve back to full-pixel CDFs when "grey" stops naming the same pixels
> on both sides, with real-pair anchors on both flanks of its ceiling),
> residual-curve knots follow the LUT's output spacing, the rotation
> census exempts sub-visible cast-inversion pass-through, and the zoned
> gate gains an absolute matched floor with an EV companion — an
> already-matched zone is left alone and says so (§4.8). v0.25.0 composed
> the photo's calibration into the closed-loop solve: every candidate
> render is the canvas's one-pass `user(base(x))`, the residual numbers
> describe exactly what the user sees (pinned to 1e-6 by a unit test)).
> The full decode →
> advise → verify → render
> pipeline ships across TWO front-ends — a native desktop GUI (`autoshop-gui`,
> egui/eframe, which links this library in-process) and the local web UI
> (`serve`) — plus the CLI, AI denoise (SCUNet sidecar), the PNG/TIFF
> baked-source mode, style retrieval, XMP sidecars (global + local masks),
> experimental generative edits, an optional pixel-**heal** retouch mode (§4.7)
> the deterministic look **reverse-fit** (§4.8) and the local server's refusal
> model (§4.9).
> 449 library + 7 CLI + 73 GUI tests pass in both build configurations.
> v0.23.3 (round 13): the XMP xmlns conflict gate resolves namespace bindings
> through an element SCOPE STACK and refuses only where a binding would
> actually corrupt this document's reading (a nested rebound island nobody
> resolves through no longer rejects the whole file); the merge-target finder
> requires the `xmlns:crs` declaration VALUE to be canonical, not just the
> attribute name; frame pops are name-matched (malformed closes degrade toward
> refusal) and a 256-live-declaration budget keeps adversarial documents from
> going quadratic. Also: the "As" parody app icon, three easter eggs (en/zh),
> and vertical-spacing fixes in the GUI toolbar and Library chips.
> v0.23.2 (round 12) split the single-file GUI into the `src/bin/gui/*`
> module tree, normalised export (save dialog + format/depth settings),
> rebuilt the settings trust model around one capability table (§3), cleared
> the 16-lane scan's entire confirmed backlog (72 findings) and closed its
> Codex review (billing-safe SSE prefix rule, adoption fenced at the develop
> lock, one sidecar read feeding both a GET's body and its ETag, full-decode
> product acceptance for denoise/segment).
> v0.23.1 (16-lane parallel scan round) landed the deferred quartet and the
> scan's verified fixes: restore-time clamp disclosure travels ALL FOUR
> `ClampSummary` fields (masks/components/curve points/string bytes) to every
> consumer; `persist_postponed` types its failures (Busy vs Io); render entry
> points construct one `ValidatedRecipe` token; the guided mask refine runs
> tiled (1024-edge tiles + 6r halo — a 61 MP refine's ~3.4 GiB of derived
> planes became ~42 MiB peak, seam-tested to ±1 grey level); bundled Python
> sidecar scripts and the weight cache resolve against the PROGRAM's own tree
> (never the cwd — an unzipped photo pack could plant `python/denoise.py`),
> `.env` cannot supply `PYTHONPATH`/`PYTHONHOME`/the weight cache (and the
> sidecars pass `-E`); every Responses-API body sends `store: false` so no
> photo persists in the key owner's account; UNC/device paths in develop-store
> sidecars are refused LEXICALLY before any filesystem probe; the local web
> server requires the session token on every POST and marks all `/api/*`
> responses `no-store`; an unreadable `variants.json` is a typed
> `Unresolved` state that the strip save primitives refuse to overwrite or
> clear (an ordinary Ctrl+S used to delete it silently); XMP import rejects
> out-of-domain curve points as a group and zeroes a colour-grade wheel's
> saturation when its hue is present but unreadable. The GUI also had its
> first-ever real-window run (user-permitted), captured in the E2E harness.
> v0.23.0 (adversarial-review round) hardened the whole crate: a per-photo
> OS develop lock (`store::with_develop_lock`, Wait/NoWait + thread-local
> reentrancy) now wraps every persistence compound across GUI, CLI and serve;
> recipe publishes are power-safe (retire-to-`.bak` + recovery at every read
> boundary); decode normalises embedded ICC profiles into sRGB (qcms — 8-bit
> direct, 16-bit via a 33³ lattice + trilinear interpolation that preserves
> bit depth); renders clamp at every public entry and load their mask rasters
> through one budgeted per-render snapshot (gate and pixels observe the same
> bytes); sidecar subprocesses are deadline-killed with bounded pipe drains;
> the GUI embeds five Noto font subsets (symbols + the Chinese UI's own
> hanzi) with a coverage gate, ships light/dark themes checked against the
> COMPOSITED colours the screen shows, and `scripts/audit_i18n.py` is a
> release gate (dynamic keys extracted from source, `tr()` bypasses flagged).
> v0.20.0 added the first RUNTIME end-to-end pass (real CLI processes over a
> real 61 MP ARW, a live `serve` hit with 25 HTTP assertions, the ambient
> `.env`/settings guards exercised as real processes against a recording mock
> endpoint, all sandboxed via `AUTOSHOP_DATA_DIR`); v0.21.0 extended it to the
> paid and sidecar paths — batch/eval/heal live, the whole serve API surface,
> and real SCUNet GPU inference. v0.22.0 (user-feedback round) persisted the
> GUI's variant strip (`variants.json` — fixes the quit-dialog livelock and
> variant-kind loss) and grew the mask system: shape composition
> (add/subtract/intersect), rotatable radials, a per-mask eye toggle,
> duplicate, a free-form brush mask, editable AI rasters
> (brush/feather/expand/contract + full-resolution guided refine) and
> LR-aligned 0-100 display scales.
> This document describes the design; a few historical **[verify]** notes are
> left in place for provenance.
>
> **Three engine rules worth knowing before reading further** (all added after
> measurement rather than review):
>
> * **A tone slider saturates; it never annihilates.** The five region sliders
>   add offsets to eight fixed knots, and past a threshold an offset overran
>   the gap to the next knot; the old repair — snap to `prev + 1e-4` — made
>   the whole interval flat, so every input tone in that band rendered to ONE
>   output value (`whites: -50` cut the top decade from 411 distinct codes to
>   75; `highlights: +60` blew 18 % of the range to pure white). Three layers
>   now hold the rule, each measured in:
>   `render::limit_tone_sliders` (v0.18.0) saturates the sliders — per slider
>   since v0.21.0: only the sliders whose contribution CLOSES the worst
>   violated interval shrink, so pinning shadows +50 while dragging whites
>   −100 keeps the rendered shadows at 50 (one λ used to pull them to 22.5),
>   with a single-λ pass kept as the unconditional backstop.
>   `render::tone_knot_weights` (v0.21.0) closes what the limiter rightly
>   skips: where EXPOSURE has already saturated both base intervals around a
>   knot (`base_gap ≤ 1e-6` — its prerogative), the basis used to add slider
>   offsets anyway, and the monotone backstop flattened the dip into an
>   interior grey band — `contrast: -100` at `+1.5 EV` flattened 197 inputs
>   at code 56304. Knot authority now follows the base curve's own local
>   separation (exactly 1 at ev = 0, so those renders are bit-for-bit
>   unchanged); a slider aimed at a clipped region yields honest clipping,
>   Lightroom's own semantics.
>   Measured on the 18-recipe × 15-exposure grid, counting only INTERIOR
>   plateaus (a run at 0 or 65535 is clipping, which a strong slider on a
>   bright frame is meant to produce): v0.19.0 shipped 13 of 270 cells over
>   96 with worst 197; v0.21.0 measures 6 cells with worst 100 — the same
>   level the ev = 0 design holds, three of the six one code below pure
>   white (65534, the quantisation edge of clipping). The old `ev > 1`
>   carve-out in the grid test is gone; one 128 bound covers the whole grid.
> * **Trust is a property of each SETTING, declared once** (`config::SETTINGS`,
>   v0.23.2). Each setting is `Secret` (authenticates and bills the user),
>   `Destination` (names where bytes go, which account pays, or what program
>   runs), or `Preference` (which model / provider / tuning number). Each
>   source is `Trusted` (the live environment; the settings file under a
>   per-user store root), `DotEnv`, or `WorkingDirFile`. One match states the
>   whole policy: `Trusted` may supply anything, a `.env` loses only
>   `Destination`, an ambient file keeps only `Preference`. Which `.env` names
>   are refused, which settings-file fields an ambient file loses, and which
>   warnings fire are all derived from that table.
>
>   **A child process's environment is the one thing NOT derived from it**, and
>   that distinction is the point. `Trust` classifies Autoshop's own settings —
>   a CLOSED set, where "not in the table" means "not a setting of ours", so
>   defaulting an unlisted name to `Preference` is safe. A child's environment
>   is an OPEN set, where "not in the table" includes every loader and
>   interpreter hook the platform defines. Reusing one predicate for both
>   answered the second question with the first one's default, and a photo
>   pack's `.env` saying `LD_PRELOAD=./evil.so` rode into both Python sidecars
>   — `ld.so` acts before `-E`, which only filters `PYTHON*`, has any say.
>   (Pre-existing; the 17-name denylist this table replaced did not list it
>   either. Codex named the shared cause during the v0.23.2 review.)
>   `config::CHILD_ENV_PASSTHROUGH` is therefore an ALLOWLIST, admitting only
>   names that select COMPUTE BEHAVIOUR — no path, no endpoint, no credential,
>   nothing that loads code — which deliberately excludes the cache knobs
>   (`HF_HOME`, `TORCH_HOME`: a redirected cache is a poisoned-model path, the
>   same reason `AUTOSHOP_DENOISE_CACHE` is `Destination`) and the proxy
>   variables (a proxy decides where bytes go). The reach this costs is small
>   and recoverable: a child INHERITS the parent's environment — nothing calls
>   `env_clear` — so a user's own `HF_HOME` or `HTTPS_PROXY` still arrives
>   untouched. Only a `.env`'s attempt to add or override one is refused, and
>   it says so.
>
>   It replaced three hand-kept lists that had provably drifted: the guard's
>   own test carried a copied 14-name array while the constant had grown to
>   17, and `Config::load` read that array BY INDEX (`pre(11)` meant
>   `AUTOSHOP_OPENAI_MODEL`), so adding or removing one name silently
>   repointed unrelated config fields at the wrong variable. The `Destination`
>   half is the one that matters most: `AUTOSHOP_CLAUDE_BIN` and
>   `AUTOSHOP_PYTHON` reach `Command::new` verbatim and the script variables
>   become that command's argv, so guarding only the base URLs left the
>   strictly worse outcome open on the same file. `ANTHROPIC_API_KEY` /
>   `_AUTH_TOKEN` / `_BASE_URL` are `Destination` too — for the `claude` child
>   the credential IS the routing decision.
>
>   Resolution is per FIELD, so a planted `autoshop.local.json` carrying only
>   `image_base_url` used to redirect the endpoint while the real key still
>   came from the environment — the filesystem twin of the cross-origin hole
>   §4.9 describes, and it needed nothing but running Autoshop inside an
>   extracted archive. Four routes to that outcome are closed: the read path
>   (v0.18.0), the settings-SAVE path — which read-merge-wrote ambient values
>   into the trusted central file, where nothing strips them again — `.env`,
>   which `dotenvy` searches for from the working directory upward, and the
>   STORE ROOT itself (v0.23.2): the per-user directory used to be
>   `%LOCALAPPDATA%` on every platform, a variable Unix does not set, so every
>   Linux/macOS build fell through to `/tmp/autoshop` and granted the settings
>   file found there full central authority. Each platform now names its own
>   per-account directory (`$XDG_DATA_HOME`, `$HOME/.local/share`), and a
>   shared-temp fallback is LABELLED (`store::RootTrust::SharedFallback`) so
>   the loader downgrades it to ambient rather than trusting it.
>
>   A `.env` keeps `Secret` on purpose — it is where this project's own key
>   lives, a documented contract — which is also why a `.env` picking
>   `AUTOSHOP_ANALYSIS_PROVIDER` is not an escalation: supplying
>   `OPENAI_API_KEY` already routes the image proposer, and that call carries
>   the PHOTO.
> * **A key that cannot ride an HTTP header is refused at the boundary**
>   (`config::header_safe_key`, v0.23.2). ureq builds the `Authorization`
>   header eagerly and quotes the whole rejected line back — key included —
>   into a `Transport` error, which then travels into rationale text, the
>   Settings status line, and any log pasted into a bug report. A trailing
>   newline from a copy/paste was enough. The value is trimmed and then
>   refused if any byte cannot appear in a header; both transport arms
>   (`post_ai_json`, `openai_models::list_models`) redact as the second layer,
>   matching what their status arms already did.
> * **A degraded save is disclosed, and the disclosure must be TRUE.** When an
>   existing sidecar cannot be merged, the note names the file whose properties
>   are lost (not the output), says whether that file was modified, and fires
>   only for a real loss — never for a blank sidecar, a baked photo's
>   neighbouring `.xmp`, or a foreign ratings/keywords file, which is now
>   spliced into rather than regenerated over (`xmp::insert_crs_description`).
>   A sidecar too large to read is itself disclosed: silently falling back to
>   Autoshop's own earlier projection made a REAL loss produce no note at all.
>
> Confirmed by the user (2026-06-25): Sony `.ARW`; output = XMP sidecar **and**
> rendered file (XMP-first); two AI roles behind one unified provider framework —
> **vision model (GPT) does image processing**, **Claude does non-image analysis
> + acceptance verification**.
>
> Since shipped, two *opt-in* pixel-level features were added alongside the
> parametric core: **AI denoise** (a Python/SCUNet GPU sidecar, run before
> tone/sharpen) and a **baked-source mode** (edit an already-exported PNG/TIFF,
> e.g. one denoised in Lightroom — auto-detected by file type). Both sidecar
> bridges share one success contract (`lib.rs::sidecar_wrote`): *exit 0 alone
> is not success* — THIS run must have produced the artifact, refusing a
> missing file, an empty file (the callers that pre-claim the output name
> create a 0-byte file first, which defeated a bare `exists()` check), and an
> untouched pre-existing file (the CLI's deterministic deliverable names mean
> a stale earlier export can sit at the path). A v0.20.0 end-to-end canary
> caught the CLI printing `denoised -> path` for a file that was never
> written; the adversarial review then broke the first fix twice more (stale
> deliverable passes; segment's `exists()` guard never fired for pre-claimed
> names), which is why the contract lives in one place with all three arms.

## 1. The core idea

The expensive, judgement-heavy part of developing a RAW photo is *deciding what
to change* (this sky is blown, those shadows are crushed, the white balance is
2°°too cool, straighten the horizon). The mechanical part is *applying* those
decisions. So we split exactly there:

```
  RAW ─► decode + features ─► [vision advisor] ─► EditRecipe ─► [Claude verify] ─► render engine
   .ARW    preview+EXIF+hist     GPT (image)        JSON          QA / accept       │
                                                                                    ▼
                                                            XMP sidecar  +  rendered image
```

**The AI never touches a single pixel.** The vision advisor receives a small
preview image + metadata and returns an [`EditRecipe`](../src/recipe.rs) — a
bounded set of Lightroom/ACR-style develop controls. A deterministic Rust engine
renders from the original RAW using that recipe. Key benefits:

- **Reproducibility** — same recipe + same RAW ⇒ byte-identical output.
- **Non-destructiveness** — the recipe is a tiny JSON; originals are never modified.
- **Auditability** — every recipe carries a `rationale` + `confidence`.
- **Lightroom interop** — the recipe serialises to an XMP sidecar, so the edit
  shows up as adjustable sliders in your catalog.
- **No hallucinated pixels** — the AI can only turn the same knobs a human would.

## 2. The `EditRecipe` contract

Defined and unit-tested in [`src/recipe.rs`](../src/recipe.rs). Adobe-convention
ranges (sliders −100..=100, exposure in stops, temperature in Kelvin). Every
field is `#[serde(default)]`, so an advisor emits only the controls it moves.
`EditRecipe::clamp()` defends the render engine. `confidence` is **advisory**,
not a gate: it is carried in the XMP comment and shown to the user, while what
actually gates auto-save is the verifier's `Verdict` — a non-Accept verdict
never writes a develop (see §4.3).

Run `cargo run -- recipe-schema` to print a default `EditRecipe` **instance**
(useful for seeing the field set and defaults). It is not the advisor's output
contract: that is a JSON Schema **generated from the control registry**
(`advisor::catalogue`, §3), and it deliberately excludes the engine-only fields
the recipe carries for calibration (`base_curve`, `as_shot_k`/`as_shot_tint`,
the lens-profile block) — an advisor must never emit those.

Local masks (`LocalAdjustment`, v0.22): base geometry (linear / rotatable
radial / bitmap raster) + optional extra **shape components** composed in
order with Lightroom's Add / Subtract / Intersect grammar
(`MaskComponent`/`MaskCombine`, rendered by `render::combined_mask_weight`),
an `enabled` eye toggle (a lossless mute — engine, coverage overlay, export
gate and XMP writer all skip a disabled mask consistently), and an optional
Range Mask refinement. Its sliders mirror the global ones plus three that
exist only per mask: `texture`, a SIGNED `sharpness` (positive sharpens,
negative softens — ACR's local band, R23-1b) and `hue`, a rotation of every
colour under the mask (R23-1b; the global mixer moves one band across the
whole frame instead). Components, `color_gains` and `role` are **engine-only**;
the radial `angle` is rendered, GUI-editable and AI-settable but is dropped by
the classic-ACR XMP projection, which carries the base geometry alone (crs
`MaskBlendMode`/`Angle` semantics have no verified reference sidecar — the
roundness rule: never reshape Lightroom masks on a guess; the writer discloses
each such loss). Bitmap rasters are immutable once referenced: every raster
edit (brush add/erase, feather, expand/contract, the full-resolution guided
refine) bakes a freshly claimed file and repoints the recipe.

## 3. The unified AI provider framework (统一 API 框架)

A Rust trait abstracts the two RECIPE-producing calls — `propose` and `verify` —
so providers are interchangeable and transport-agnostic (an HTTP API, or
shelling out to the `claude` CLI). Two roles, each independently configurable in
the in-app **Settings (⚙)** panel. (The later pixel-side AI calls — generative
reimagine/retouch, heal spot-detection, style-prompt extraction, the denoise and
segmentation sidecars — talk to their endpoints directly rather than through
this trait.)

| Role | Provider options | Sees pixels? | Job |
|------|------------------|--------------|-----|
| **Image advisor** (图像) | OpenAI-compatible vision over HTTP (default `gpt-5.5`); Settings offers **API** (a real key) or **OAuth** (a local Codex bridge fronting a ChatGPT subscription) | **yes** (preview) | Look at the photo → emit an `EditRecipe`. The `claude` CLI has no image input in print mode, so this role never uses the claude OAuth path. |
| **Analyst / verifier** (分析) | **OAuth** (`claude` CLI, default model `opus`) **or** API (OpenAI-compatible chat) | **no** (data only) | Reason over EXIF/histogram; **acceptance-verify** the recipe (ranges sane? consistent with metadata & intent? confidence adequate?) and flag/veto bad recipes. |
| **Visual judge** (R20, `advisor::judge` — reuses the image role's endpoint/key, not a third credential) | same OpenAI-compatible vision endpoint as the image role | **yes** (reference + candidate renders) | Score a RESULT against a reference (strict-schema `{score, decision, critique, hint}`). Two consumers: the analyze closed loop (proposal rendered → judged → one guided revision, adopted only on a ≥ re-score) and the reverse-fit's opt-in AI review (target vs fitted render). Since R23-6 the fit review's `hint` is SHOWN (R20 dropped it silently) and, with the opt-in `deep` switch, is read by `advisor::hint_action` as a CHOICE among the app's own bounded moves (`FitAction`) — never as a parameter value, which is why the closed list lives beside the reply it reads. |

> The verifier judges at the **data level** — recipe + histogram/clipping stats +
> the advisor's rationale — *without* re-doing vision. The R20 judge is the
> complementary eye: it sees only PIXELS (two JPEG renders), runs at the END
> of the look-mutation chain (post style-distillation, WB-anchored by the one
> hoisted calibration snapshot, refine masks carried into the render clone),
> and every adopt/keep/failure branch discloses through the rationale. It is
> a paid vision call, so it is an explicit caller decision: interactive
> analyze surfaces pass `judge = true`; `batch` and `eval` pass `false`.
>
> **Ordering, and its one revision.** R20 decided the REVERSE-FIT's review
> runs AFTER the persist — informational, never gating, every failure a
> status note. That is still the default path and is unchanged. R23-6 revises
> it for one explicitly-chosen case (user decision 2026-08-17 ⑥): with 「deep」
> ticked, the review runs BEFORE the persist and may buy ONE guided retry,
> because a reviewer cannot act on a recipe that is already on disk. The
> analyze loop's ordering is untouched, and the CLI needed none — `match` has
> always evaluated before it writes.

> **The control registry is the single source of the AI contract** (R23-1,
> [`src/advisor/catalogue.rs`](../src/advisor/catalogue.rs)): `RECIPE_CONTROLS` /
> `LOCAL_CONTROLS` list every develop control with its range, neutral value,
> engine-only flag, `crs` key and one-line purpose, and everything downstream is
> DERIVED from them — the strict response schema (both mirrors), the proposer
> prompt's control catalogue, the eval ruler and the style index's reference key
> set. `EditRecipe` is still the data contract; the registry is what keeps the AI
> side and the measuring side from silently falling behind it (a field added to
> the recipe without a registry row does not compile).
>
> R23-1b emptied the registry's engine-only column of everything that was merely
> *staged*: the three manual lens controls entered the schema as
> `["number","null"]`, where **null means "no opinion"** and 0 means "clear the
> photographer's correction" — a distinction `pipeline::carry_over_unrepresentable`
> now honours (it used to overwrite all three unconditionally, which would have
> made the schema addition a no-op on the Refine path). What is left engine-only
> is permanent: the engine's own per-photo measurement (`as_shot_k`/`as_shot_tint`,
> `base_curve`, the lens-profile block) and mask state with no ACR spelling
> (`components`, `color_gains`, `role`, plus the `enabled` eye toggle, whose only
> possible model answers are "discarded" or "mute the user's mask").

### 3.1 The grade-strength axis — one dial, six gates (R23-3)

How hard the AI pushes is a first-class user input: `recipe::GradeStrength`
(0..1) — the GUI's **Strength** slider, the CLI's `--strength`, the web body's
`grade_strength`. It has two NAMED points: `0.50` is the point every restraint
NUMBER in the app was tuned at (the 147-photo eval of `f944ef3` plus
`bd3f9d4`'s highlight-integrity cases) — the ±50/±35 guardrail pair and
`temper`'s knees are bit-for-bit what shipped up to v0.28.0. It is **not** a
"behave like the old release" switch: `0.50` lands in the *balanced* band, while
the verbatim restraint wording those releases sent is now the *restrained*
(`≤ 0.40`) prose, whose soft-cap factor is already 0.93 — so no single strength
reproduces a pre-R23 request in its entirety. `0.65` is the shipped default
(user decision 2026-08-17 — "a bit braver than today, with one click back to the
calibration point").

The value is **deliberately not an `EditRecipe` field**. It is intent for one
analysis, not a develop parameter: in the recipe it would have to be projected
into a Lightroom XMP contract that has no such notion, and it would change
`store::recipe_norm`'s structural fingerprint — the R21 deleted-version registry
documents that a schema drift there fails *open*, so every already-recorded
deletion would lose its structure arm.

Why it is an axis and not a patch: **six** independent places decided "how hard
to push", each a hard-coded constant, and five of them getting braver while one
stays fixed produces a develop exactly as timid as before (the verifier revises it
back, or `temper` compresses it back). All six read the dial.

| # | Gate | Where | What the dial changes |
|---|------|-------|-----------------------|
| 1 | Proposer prompt | `advisor::openai::{strength_clause, guardrail_pair, look_coverage_clause, mixer_restraint_clause}` | Three banded restraint templates; the quoted ±Highlights/Shadows and ±Whites/Blacks pair opens from the measured ±50/±35 up to ±75/±55; "most photos need only a couple of HSL bands" becomes an explicit per-control decision |
| 2 | `EditRecipe::temper` | [`src/recipe.rs`](../src/recipe.rs) | The four soft-cap knees/ceilings scale by `1 + (s − 0.5)·0.7` — 0.5 is the shipped 50→70 / 30→45 exactly, and full strength asymptotes at 94.5, still inside the ±100 hard `clamp` |
| 3 | Verifier prompt | `advisor::{verify_flat_clause, verify_cooked_clause}` | The too-FLAT band tightens, the OVER-COOKED band relaxes; the target and the photographer's DIRECTION are stated ABOVE the checklist they modify |
| 4 | Visual judge rubric | `advisor::judge::intent_rubric` | The Develop rubric gains the target and the direction. FitMatch gains neither — a look MATCH has no strength |
| 5 | Style reference wording | `style::render_reference` (+ gate 1's reference clause) | Below the committed band the retrieved habit is a CEILING ("not stronger", "do not exceed it"); at it, a FLOOR. The measured NUMBERS never change — a dial must not restate what the photographer actually did |
| 6 | No-AI fallback | `advisor::heuristic` | The baseline's histogram-driven recovery goes through the same `temper` dial, so the fallback cannot taste different from the AI path at one setting |

Bands are coarse (≤ 0.4 restrained / ≤ 0.7 balanced / above committed) because
prose cannot be interpolated; every NUMBER on the axis is continuous. One
consequence worth knowing: 0.50 and 0.65 share the balanced band, so they differ
in the guardrail numbers and `temper`'s knees, not in the adjectives.

Two things the axis must never touch, both measured defects rather than taste:
`temper`'s **white-point coupling** (`whites ← −highlights·0.3`, global and per
mask — `bd3f9d4` fixed a recipe that dragged sea foam to grey), and the prompt's
matching "recovering highlights must NOT grey out specular whites" rule, which
stays unconditional at every strength. `clamp`'s hard ranges are a safety bound
and are not on the axis either. The **Style** slider's `blend_toward` cap (0.6) is
off the axis on purpose too: that number bounds mean-regression toward the user's
own average edit, so coupling it to strength would turn "push harder" into "look
more like my average" — the other axis, pointing the other way.

This also closes a gap this document had already declared: the verifier row above
has always promised "consistent with metadata **& intent**", while
`build_verify_prompt` took `(recipe, meta, hist)` and could not see the intent at
all. It now takes an `advisor::GradeIntent` — strength plus the photographer's
own direction (the RAW one, never the Refine envelope, which embeds a whole
recipe the reviewer is already reading).

Sketch (final shape):

```rust
trait Advisor {                      // one trait, many providers
    // `ProposeContext` / `GradeIntent` carry the per-call intent (style
    // reference, direction, revision hint, WB anchor, grade strength) so a new
    // input cannot be silently dropped by a call site that still compiles.
    fn propose(&self, img: &Preview, meta: &Meta, hist: &Histogram,
               ctx: &ProposeContext) -> Result<EditRecipe>;                // image role
    fn verify(&self, recipe: &EditRecipe, meta: &Meta, hist: &Histogram,
              intent: &GradeIntent) -> Result<Verdict>;                    // analyst role
}
// propose: OpenAiProvider (HTTP vision)        |  HeuristicProposer (no-key baseline)
// verify:  ClaudeProvider (claude CLI -p OAuth) |  OpenAiVerifier (HTTP chat, OpenAI-compatible)
```

Provider/model/key selection lives in `autoshop.local.json` (written by the
Settings panel) and/or `.env` — both gitignored; the local file overrides env.
That file lives in the per-user store root, not beside the checkout, so settings
do not depend on which directory the app was launched from (a cwd-relative
`autoshop.local.json` is still read as a legacy fallback).
The OAuth analysis path reuses Claude Code OAuth — **no API key needed**; the
image path and the API analysis path each need an OpenAI-compatible key.

## 4. Components & milestones

| ID | Component | Crate/tool (actual) | Status |
|----|-----------|---------------------|--------|
| M0 | Data model + CLI scaffold | `clap`, `serde`, `serde_json`, `anyhow`, `thiserror` | **done** |
| M1 | RAW decode + features (Sony ARW) | **`rawler` 0.7.2** (preview + EXIF + WB) | **done** |
| M1 | Unified provider framework + GPT advisor + Claude verifier | `ureq` (HTTP) + `claude` CLI | **done** |
| M2 | Deterministic render engine | `image`, custom tone/colour/WB/clarity/NR/sharpen ops | **done** |
| M2 | XMP sidecar writer (ACR `crs:`, global + local masks) | hand-rolled XML | **done** |
| M3 | `auto` end-to-end + batch | batch fixes its work list up front, then runs it through a bounded pool (≤3 worker threads); "pending" = no develop in the store (recipe.json or `<stem>.xmp`, central or legacy) | **done** |
| M4 | Style retrieval + eval harness (your edits as ground truth) | k-NN over EXIF+histogram; per-field MAE/bias | **done** |
| M5 | Local web UI | `tiny_http` + vanilla JS (gallery, live before/after) | **done** |
| V2 | AI denoise (high-ISO/astro) | Python sidecar → **SCUNet** on GPU, called from Rust | **done** |
| V2 | Baked-source mode (edit exported PNG/TIFF) | extension dispatch; develop runs on loaded pixels | **done** |
| V2 | Generative reimagine / retouch | OpenAI Images (`gpt-image-*`) | **done (experimental)** |
| V2 | Pixel retouch / heal (spot removal) | deterministic heal engine + vision spot-detect ([`src/retouch.rs`](../src/retouch.rs)) | **done (experimental)** |
| V2 | Look matching / reverse-fit (`match`) | distribution-level solve for the recipe that reproduces a target rendition ([`src/fit.rs`](../src/fit.rs); zoned variant [`src/fit_zoned.rs`](../src/fit_zoned.rs)) | **done** |

### 4.1 RAW decode (M1)

Backed by **`rawler` 0.7.2** (chosen over the now-frozen `rawloader` for current
Sony body coverage + embedded preview + full EXIF; see [`src/decode.rs`](../src/decode.rs)).
It extracts the embedded JPEG preview (for the vision advisor + UI), a downscaled
histogram with clipping stats, and EXIF (camera/lens/ISO/shutter/aperture/
as-shot WB). Baked sources (PNG/TIFF/JPEG) skip this and load directly via the
`image` crate with neutral metadata.

**One dispatch, enforced at the gate (R22).** "A RAW" has a single definition
app-wide (`decode::is_raw`), and the two ways into pixels are separate by
construction: a RAW must be demosaiced by the develop engine, a baked raster is
decoded by the `image` crate. `decode::load_image` therefore **refuses a RAW by
name** instead of failing later as an unrecognised format, and any consumer that
may hold either kind asks `render::source_pixels(path, cap)` — the one two-armed
branch (RAW → neutral `render_to_image` at `cap`; baked → `load_image`,
thumbnailed to `cap` and only ever DOWN). A lib test patrols the remaining
`load_image` call sites so a new consumer cannot re-copy the branch: hand-copying
it is what silently broke full-resolution AI mask refine in v0.22. The patrol is
per CALL SITE — every line naming the gate carries `// baked-by-construction:
<why>` (or `// not-a-consumer-call:` for the gate's own declaration and tests),
and an unmarked line fails the build. It used to assert a FILE allow-list, which
let an already-listed file — including the v0.22 accident site itself — grow a new
hand-rolled decode unnoticed. R23-6 moved two more consumers onto the gate: both
reverse-fit entries (`match`'s `--target` and the desktop reference picker) now
accept a RAW reference through `source_pixels` rather than refusing it by name —
the CLI prints the one thing that makes it honest, that a RAW reference is
developed NEUTRALLY and its own develop settings are not read.

### 4.2 Vision advisor — image processing (M1)

A vision-capable OpenAI model receives the preview + metadata and returns an
`EditRecipe`. What M1 left as **[verify]** is settled by the shipped code
([`src/advisor/openai.rs`](../src/advisor/openai.rs)): the Responses API with a
strict `json_schema` for structured output, the preview sent as a base64
`input_image` data URL, and the model id read from config (default `gpt-5.5`) —
never hardcoded from memory.

### 4.3 Claude analyst / verifier (M1)

The verifier (analysis role) runs one of two providers (set in Settings):
**OAuth** — the `claude` CLI for non-image reasoning + acceptance verification via
`claude -p --setting-sources "" --strict-mcp-config --disable-slash-commands
--output-format json --model <opus>` (default `opus`), reusing Claude Code
OAuth — no API key. `--bare` is deliberately NOT used: since CLI ≥ 2.1.210 it
never reads the stored OAuth login ("OAuth and keychain are never read"), which
is exactly the v0.11.2 auth failure — see `src/advisor/claude.rs`; or
**API** — an OpenAI-compatible chat endpoint (`OpenAiVerifier`, `/chat/completions`)
sharing the same data-only prompt. Either returns a `Verdict` (accept / revise /
reject + reasons). A rejected recipe can trigger one revision round with the
vision advisor.

### 4.4 Render engine (M2)

Applies the recipe deterministically. Frame stage first: decode → EXIF
orientation → working-resolution cap → lens geometry (distortion/CA) →
straighten → crop. Then the pixel stage, in this order: anchored white balance →
lens-profile vignette → manual vignette → **dehaze in linear light** (before any
tonal work, so the airlight estimate cannot move when Exposure is dragged) →
tone LUT (exposure/contrast/whites/blacks/highlights/shadows, the tone curve and
the per-photo camera base curve composed into one table) → per-channel RGB
curves → 8-band HSL → colour grading → clarity → saturation/vibrance → noise
reduction → sharpening → local adjustments (linear/radial/bitmap masks).

Each mask runs its own sub-chain, in this order: **dehaze** → the fused
**WB + tone + saturation** blend → **clarity** → **texture** → **noise
reduction**. Clarity/dehaze/texture became engine-rendered in R22 (feedback
#15a/#10B — until then they were carried, exported to XMP and drawn only by
Lightroom, so a mask that moved only those three did nothing in-app; recipes
saved before R22 re-render with the local effect, which the user signed off on).
Local dehaze shares the global haze model, with the airlight estimated once per
frame so mask order cannot change it; clarity and texture are the same
mask-weighted unsharp operator at a large midtone-masked radius and a small
plain one. Two deliberate residues vs the global order are documented at
`render::apply_masks`: local Temp/Tint lands after local dehaze, and local
saturation before local clarity/texture — splitting the fused WB/tone/saturation
blend to fix either would change the output of every existing partial-weight
mask.

Export adds the colour-space encode (sRGB / Adobe RGB / Display P3 / ProPhoto,
with a wide-gamut develop path), an optional long-edge resize (Lanczos3, never
upscales) and resolution-aware sharpening, then writes JPEG, TIFF or **PNG** —
PNG is a first-class 16-bit master here, since every heal / clone / denoise /
reimagine master is one.

### 4.5 XMP sidecar (M2) — primary deliverable

The recipe written as an ACR/Lightroom `.xmp` sidecar (`crs:` keys like
`Exposure2012`, `Contrast2012`, `Temperature`, `ToneCurvePV2012`), so the AI's
edit appears as fully-adjustable sliders in Lightroom — the "AI does 90%, I
nudge the last 10%" workflow.

Since v0.13.0 Autoshop does **not** write it next to the `.ARW`: the source
library is read-only, so the projection lands in the per-user develop store
(`<AUTOSHOP_DATA_DIR | %LOCALAPPDATA%/autoshop | $XDG_DATA_HOME/autoshop |
$HOME/.local/share/autoshop>/develops/<stem>-<hash of the absolute
path>/<stem>.xmp` — see `store::store_root_with_trust`, and the trust bullet in
§3 for why the shared-temp last resort is labelled rather than
trusted), alongside `recipe.json` (the authoritative develop
state), version snapshots (a deleted snapshot registers its number + content
fingerprints in the develop's permanent `.deleted-versions.json`: the number
is never re-issued and the backup gate stops auto-preserving the discarded
content — a discard record, not a recovery copy), mask rasters, `pixels.json`
(the baked pixel-master
link) and, since v0.22.0, `variants.json` — the GUI's variant strip
(background variants' kind/recipe/raster origin + the active card's
three-valued kind), which is what lets a 「反推 Reverse-fit」 or 「AI 生成」
card survive a reopen and lets the quit dialog's Save-all genuinely save
background variants instead of livelocking. Copy the XMP beside the RAW when
you want Lightroom to pick it up. A Lightroom sidecar that already sits beside
the RAW is READ on open — the newer intent wins — and never overwritten (mask
corrections it carries that classic import can't represent — brush / AI /
depth — are counted and disclosed, not silently dropped).

The **export** direction is disclosed the same way (M6a). Classic ACR XMP
cannot express everything the engine renders, so the writer names what it left
behind while it emits: raster (bitmap) and muted masks are skipped whole, extra
Add/Subtract/Intersect shapes flatten to the base geometry, a rotated radial
exports unrotated, and per-channel recolour gains do not travel. The verdicts are
the writer's own, produced by the ONE loop that emits the mask block (so the claim
cannot drift from the file) and handed back with the document itself —
`xmp::recipe_to_xmp_with_losses` / `MergeOutcome::losses`, one pass per save;
`xmp::mask_export_losses` is the same list for a surface that only wants the
disclosure and writes nothing. It rides `pipeline::write_xmp`'s return value to
every surface: the GUI localises it into the save status line and a toast, the
web reply carries it in the note it already had, and stderr gets one line from
`write_xmp_doc` for the CLI. `recipe.json` remains the lossless sidecar, which
is what the disclosure says.

### 4.6 Style / eval harness (M4)

The user's **finished edits** are ground truth. If they're Lightroom XMP/develop
settings, diff the AI recipe against them; if they're exported JPEGs, compare the
AI render perceptually. Lets us measure "does the AI match *how the user*
develops a shot?" and tune the advisor prompt accordingly.

**Three surfaces, one loader (R23-2).** The library
([`src/style.rs`](../src/style.rs)) is built from RAW+`.xmp` pairs by
`autoshop style-index <dir>`, by the web info panel, and — since R23-2 — by the
GUI's **AI panel › Style reference library** (folder picker → background worker
with per-photo progress; no cancel, because `StyleIndex::build` has no
cancellation checkpoints, so the button simply stays disabled until it lands).
The index always publishes to the per-user store (`store::style_index_path`);
the legacy cwd-relative `out/style-index.json` stays readable. Reading it is
`style::load_effective` / `style::index_info` — ONE central-then-legacy walk
returning three typed states (`Loaded` / `Absent` / `Unusable`), consumed by the
pipeline, the web handler and the GUI status line alike. The empty-index refusal
lives in `StyleIndex::save`, so no caller can truncate a good index with a
failed build.

Two disclosure rules follow from those states. A develop that ASKED for style
and ended with no reference always says so in the rationale — the condition is
"strength > 0 and the final reference is `None`", which covers a missing
library, a retrieval that matched nothing and an unusable file in one place
(before R23-2 only the last of the three spoke, so a fresh install had an inert
Style slider and no explanation). And every analysis that DID get a reference
names the shots it used, bounded to file stems. The optional
reference-**image** switch (GUI only, off by default) additionally sends the
nearest past photo as a second `input_image` on the propose call — the prompt
names the two frames by position, `store:false` covers both, and the extra image
is disclosed in the tooltip and in the rationale.

### 4.7 Pixel retouch / heal (optional) — V2

A third, opt-in editing mode (`autoshop heal`, or the UI's **修图 · 去瑕疵** panel),
distinct from BOTH the parametric path (which never touches pixels) and the
generative path (which *synthesises* them). It does traditional **spot-healing**:
small defects (dust, sensor spots, blemishes, specks) are removed by sampling
SURROUNDING REAL pixels and blending them over the defect with a mean-corrected,
feathered patch (the "heal" vs "clone" distinction). By construction the engine
only ever copies / shifts / averages pixels that ALREADY exist — it never invents
content, so this stays *retouching, not generation* (the hard design constraint).

Targeting is hybrid: a vision model auto-detects small spots
([`detect_spots`](../src/retouch.rs), constrained by prompt + schema to small
spot-removals) and/or the user paints regions in the UI
([`plan_from_mask`](../src/retouch.rs) → connected components → circular targets);
both feed the deterministic [`heal_image`](../src/retouch.rs) engine. Donors are
auto-searched (the in-bounds neighbour whose surroundings best match the spot's
border) unless an explicit source offset is given. Output is a pixel master in
./out — **non-XMP** (pixel edits don't serialise to ACR) — and the develop
records it as its pixel source in `<store>/develops/<key>/pixels.json`, so every
later render, export and reopen applies the parametric recipe ON TOP of the
healed pixels instead of silently reverting to the untouched source. It runs on
the engine's own neutral develop for a RAW (a ≤2048px cap by default) or the
source thumbnailed to 2048px for a baked image — never the camera's baked
preview, so the healed master stays on the same tone chain as the canvas
develop; `--full-res` works at full resolution on either source type (slow).

### 4.8 Look matching / reverse-fit (`match`) — V2

The inverse of the advisor path: given the same frame twice — the untouched
source and a *target rendition* of it (a `reimagine` output, an exported JPEG,
any finished reference of that shot) — solve for the `EditRecipe` that
reproduces the target through our own engine ([`src/fit.rs`](../src/fit.rs)).
No pixels are copied, so the answer is sliders + curves: it applies at full
sensor resolution and serialises to XMP like any other develop. Deterministic
and key-free.

The method is deliberately **distribution-level, not per-pixel regression** — a
generative target is not pixel-aligned with its source, so only statistics are
trustworthy. Three stages, in this order: luminance-CDF tone matching (sampled
at the engine's own tone knots and least-squares solved against the engine's own
slider basis, with a ridge + penalised model-selection prior so numerically
equivalent but semantically ruinous slider combos lose); then saturation by
mean-chroma ratio, secant-refined through real renders and closed with a
do-no-harm check; then per-channel CDF residuals as red/green/blue curves,
admitted only through three vetoes (aggregate error, foreign-hue, rotation
budget) — each veto is a specific real-photo failure recorded at its const
block.

The tone stage's evidence prefers NEAR-NEUTRAL pixels (saturated ones carry
chroma-clipped luma), which rests on an identification assumption — "grey"
must name the same pixels on both sides. Since v0.26.0 (R17) that assumption
is policed by three conditions in `tone_cdf_pair`: per-side sample floors, the
1.75× share-ratio ceiling, and a **misprediction gate**
(`neutral_gate_misprediction`) that scores the gated evidence map against the
shared class's own empirical pairing — the one exception to the
statistics-only rule, and a deliberate one: it reads coarse CO-MEMBERSHIP at
equal thumbnail index (same-frame pairs on the same 384-edge grid) as a
diagnostic, never as pixel evidence, and it fails OPEN — under broken
registration the metric slides toward 0 and the older gates stand alone (the
share ratio is alignment-free, which is why both detectors stay: measured on
the archive, one real pair is caught only by the misprediction gate and the
misaligned class only by the share gate).
Any condition failing falls the solve back to full-pixel CDFs on both sides.
This is what un-murked the live pair the share gate had waved through (its
target re-hued a quarter of the source's neutral class — the pale sky — into
vivid blue; the gated map then darkened every upper-mid by up to 22/255 while
the scalar claimed victory). The residual tone curve places its knots
uniformly in the LUT's OUTPUT domain (inverted through the LUT) rather than
uniformly in raw input, because the curve's input axis is the engine's output
and a steep camera base compresses fixed-x samples into 38-u8 gaps whose
piecewise-linear chords sag ~10/255. `--zoned` ([`src/fit_zoned.rs`](../src/fit_zoned.rs)) adds a sky-to-sky
local correction on top of the global fit via the segmentation sidecar; the
XMP carries the global fit only, since classic sidecars cannot hold raster
masks. Each zone's correction is judged zone-locally by a two-arm gate
(v0.26.1): halve the zone error, or land it at/below an absolute matched
floor (0.02 of linear-mean error, brightness within a quarter stop — the
floor lives in scale-dependent linear light, so the EV companion rides
both absolute yardsticks) with a real ≥20% gain. A zone already inside the
observed matched DOMAIN (≤0.012, same EV companion) is left alone with an
honest "already matches" note instead of being dialled, regressed, and
reported as a dropped improvement; zones between the two yardsticks are
always attempted — the skip line and the acceptance floor are split
constants precisely so nothing fixable is declined untried. The GUI's **反推 / Reverse-fit** action drives the
same two entry points (`fit_recipe`, `fit_recipe_zoned`) and lands the
result as an editable variant.

**The reference, and the second reading (v0.29.0, R23-6).** The desktop
target is no longer only an app-generated variant: `canvas::fit_target`
prefers an explicitly chosen `fit_ref` (「Choose reference…」 — any finished
rendition of the same frame; a RAW goes through `render::source_pixels` and is
developed NEUTRALLY, which the CLI says out loud) and falls back to the active
generated variant. Both entries coexist, and the CLI has always accepted an
arbitrary path. A cheap same-frame check (`fit::same_frame_plausible`, aspect
within 2%) WARNS — never refuses — when the reference is not this shot.
Alongside the frame-global `look_err`, every fit now carries a **joint
value-range reading** (`fit_zoned::joint_reading`): four luminance bands ×
two chroma classes, built on the existing `zone_moments` weight-vector
interface, compared with `zone_err`'s formula read in the DISPLAY domain so
one number means the same thing in shadows and highlights. It answers a
question no term of `look_err` computes — its colour term is three
*unconditional* channel means and its hue term skips everything under 0.06
chroma — and it has exactly three jobs: report; cap the reported confidence
(downward only, never raise); and act as ONE additional bounded-drift veto at
the pipeline END (`fit::terminal_harm`), fail-open, in
`ZONE_GLOBAL_REGRESSION_TOL`'s shape. It is deliberately NOT a per-stage gate:
measured on this repo's own fixtures, the bucket a change fixes loses its
members to a neighbour, so a per-stage worst-bucket comparison rejects the one
correct cast in the set and admits both wrecks
(`fit::tests::the_worst_bucket_cannot_gate_a_stage`). It is never mixed into
`look_err`'s weighted sum — R17-R19's constants were each calibrated against a
real failure pair. `AUTOSHOP_FIT_JOINT=off` removes the whole family for
baseline comparison. A zoned fit's **confidence** now comes from the zones it
actually accepted rather than the frame-global number this module's own
acceptance doc proves cannot judge a zone; `err_after` keeps its contract
(frame-global, comparable to `err_before`) and the rationale says which is
which.

Since v0.25.0 the GUI drives both entry points through a **composed
calibration base** (`fit_recipe_from` / `fit_recipe_zoned_from` with
`pipeline::calibration_recipe`): the solve's recipe STARTS from a
calibration-only recipe — base curve, lens profile, as-shot anchors, from
a saved-first authority that falls through to a fresh estimate when the
saved calibration is all-neutral (`pipeline::fit_calibration`; an earlier
UNSTAMPED fit recipe would otherwise poison the authority with its empty
curve) — so every closed-loop candidate render IS the canvas's one-pass
`user(base(x))` and the deliverable carries the calibration by
construction. Fitting from the raw neutral spent the bounded model (the
±60 saturation cap, the hue-rotation budget, the slider ranges)
re-deriving the 0.6–1.4 EV camera look before the actual grade got a say —
measured on a real pair, the neutral-source solve pegged saturation and
had its cast curves vetoed. The tone stage solves its sliders in the USER
domain (their input is the base curve's output) and rebases the residual
curve through the base LUT; statistics are measured against the base
render, so `err_before` means "calibration look vs target". The reported
residual equals a recompute of the canvas's DEVELOP pass to 1e-6
(unit-pinned; the GUI's later lens-geometry resample sits outside the
fit's model — pre-existing, second-order), which retired v0.24.0's
two-pass seed and its `scale_chroma` clamp-order gap (measured up to
~18.7/255 mean on saturated fixtures). The CLI `match`
keeps its embedded-preview + post-stamp contract
(`pipeline::stamp_fit_calibration`) — `preview_only` needs no demosaic,
and that command's contract was validated on it.

One deliberate asymmetry: the fit does **not** apply
`render::limit_tone_sliders` to its proposal, even though the engine applies it
when rendering. The solve is a linear inversion of the knot model, and the
acceptance test downstream is a knife edge — on the hazy-to-clean fixture as
it stood pre-R17 (gated evidence, sparse residual knots) the solved recipe was
only 3 % better than neutral (0.08625 against 0.08918), so a 0.34 % nudge to
the sliders pushed it over `err_before`, tripped the saturation do-no-harm
loop, and ended at 0.1286, far worse than doing nothing. (R17's evidence
fallback moved that fixture to 0.0892 → 0.0229; the historical figures stay
because the knife-edge geometry, not the exact numbers, is why the asymmetry
exists.) The fit does not need to predict the limiter, because it scores
candidates by **rendering** them: it already measures whatever the engine
actually does.

### 4.9 What the local web server refuses

`serve` binds `127.0.0.1`, and that on its own protects nothing: any page the
user visits can POST to a loopback port without a preflight, and a DNS-rebinding
name can read the answers too. The threat model is therefore *a hostile page in
the user's own browser*, and the guarantees are:

- **Cross-origin refusal** ([`foreign_origin`](../src/serve.rs)) runs before
  dispatch, so it covers every route. `Host` and `Origin` must be a literal
  loopback authority **on our port** — an absent port means the scheme default
  (80), not "any port", or a page served on loopback:80 would count as
  same-origin and could repoint the AI base URL through `/api/settings`, after
  which the next Analyze hands over the user's API key.
- **The UI is not frameable** (`X-Frame-Options: DENY`): the port is fixed and
  well known, and a framed click is same-origin by construction.
- **Bounded inputs.** JSON bodies cap at 256 MiB, uploads at 500 MB, SSE
  assembly at 64 MiB for text and 512 MiB for images, and XMP sidecars —
  metadata the user *receives* — at 16 MiB through one reader
  ([`store::read_sidecar_checked`](../src/store.rs), which reads BYTES and
  checks the size before validating UTF-8, so an over-cap sidecar is never
  reported as "not readable text"). Responses we *read back* from the AI
  endpoint are capped too ([`advisor::into_json_capped_at`](../src/advisor/mod.rs)):
  the blocking JSON arms used `into_json`, which reads until the server stops,
  and an allocation failure aborts the process rather than raising an error.
  The cap is the **caller's** — an images/edits response carries a base64 frame
  roughly the size of the whole text budget — and the reader preserves ureq's
  `TimedOut` kind, because the caller branches on it to decide whether to warn
  that an accepted (2xx) request may already have been billed.
- **Bounded time, measured in CONTENT.** A streaming call runs under an
  inactivity deadline rather than a total one, because a healthy long
  generation proves liveness through events. But the socket's stall timer
  re-arms on *bytes*, and the SSE keep-alive idiom is a comment line — so a
  server or proxy sending only `: keep-alive` held a worker forever, and the
  per-event cancel check never ran because no event ever arrived.
  `for_each_sse_json`'s progress gate therefore re-arms on stream **content**
  only: comment and blank lines do not count, while `event:` / `id:` lines and
  every byte of a multi-megabyte `data:` line do.
- **Work the caller controls is scanned once.** The multipart boundary must not
  occur in any part (RFC 2046 §5.1.1), and the first version of that guard
  extended the candidate one character at a time, rescanning every part — a
  quadratic in a value the caller supplies, reachable through an uncapped
  `prompt`. `generative::choose_boundary` makes one pass, jumps straight past
  the longest run, and refuses past the 70-character ceiling rather than
  sending a body whose delimiter the content could forge.
- **Bounded work.** `EditRecipe::clamp` caps masks/curves/knots **and strings**
  (`rationale`, mask names, bitmap-mask paths), and the cap is enforced at
  **both** persistence boundaries — `pipeline::write_recipe` and
  `write_xmp_doc` each clamp the copy they write — not only on the render
  routes. A recipe that reached disk used to escape every later reader's
  expectations, and the XMP half was protected only by the convention that
  every caller clamps first, while it is the half that lands beside the RAW in
  the user's library. The strings mattered on their own: an upstream HTTP error
  body reaches `rationale` verbatim, so a merely broken endpoint could write
  megabytes into `recipe.json` and into the XMP.
- **Clamping never decides a deletion.** `POST /api/xmp` treats a neutral
  recipe as "clear my edits" and unlinks the develop, so that branch is decided
  on the recipe **as sent**, before clamping. Clamp drops whole components
  (a zero-area crop becomes `None`), so a body carrying only a degenerate crop
  clamped to exactly `EditRecipe::default()` and deleted saved work that the
  same request would have *saved* before the clamp was added.
- **Two tabs cannot silently destroy each other's saves.** `GET /api/recipe`
  answers with an `ETag` naming the saved *develop* (`store::develop_revision`):
  the recipe.json bytes (`"none"` when no save exists — absence is a real
  revision, so two tabs racing the *first* save still collide), **with the
  Lightroom sidecar's content hash folded in whenever that sidecar currently
  out-ranks the store** — it is then the very body the GET serves, and tagging
  only the store file let a stale tab pass the precondition while the sidecar
  its answer came from had changed (round-12 L04-4). `POST /api/xmp` honours
  `If-Match` against the same tag: a stale one gets **412** and writes nothing
  — the clear branch included, since a clear destroys a lost update just as
  surely. The bundled client always sends the tag it loaded and adopts the one
  each save/analyze answers with (the save reply computes its tag *after* the
  projection write, so the adopted tag describes the compared state); a
  request without the header (curl, the CLI, older pages) stays unconditional.
  pixels.json and mask rasters stay outside the tag by design.
- **Bounded concurrency, released on unwind.** Eight request slots, each held
  by an RAII permit: a handler that panics on a malformed image still gives its
  slot back, where the earlier tail-release leaked one per panic and wedged the
  accept loop after eight.
- **Header values carry no control bytes**, enforced in the constructor rather
  than by each call site remembering to percent-encode.
- Keys are never returned: `GET /api/settings` answers `key_present: bool`.

## 5. Why Rust

Cross-platform, no GC pauses on large-image pipelines, first-class image crates,
single-binary distribution, trivial `std::process` shell-out to `claude`.
Toolchain in use: rustc/cargo **1.94.1** (verified locally).

## 6. Open questions

| # | Question | Status |
|---|----------|--------|
| 1 | **Image library path** (originals + finished edits) | resolved: passed per invocation (`batch <dir>`, `serve --dir`, `style-index <dir>`, the GUI folder picker) — no configured library root; develop state is keyed by each photo's absolute path in the per-user store. One exception since R23-2: the STYLE library's source folder is remembered in the GUI prefs (`style_src_dir`) so a rebuild need not re-find it — a convenience, not a library root; the index records the folder it was built from as well |
| 2 | Camera / RAW format | resolved: Sony `.ARW` |
| 3 | Output target | resolved: XMP sidecar **+** rendered, XMP-first |
| 4 | AI roles | resolved: GPT=image, Claude=non-image+verify, unified framework |
| 5 | Exact meaning of Claude's "收货验证" (data-level vs pixel-level) | resolved: **data-level**. The verifier is never sent pixels — it judges the recipe against EXIF/histogram/clipping stats and the advisor's rationale (§3, §4.3, [`src/advisor/claude.rs`](../src/advisor/claude.rs)) |
| 6 | How to feed the preview to the GPT vision API; `crs:` key set for ARW | resolved in shipped code: the preview goes as a base64 `input_image` data URL on the Responses API with a strict `json_schema` ([`src/advisor/openai.rs`](../src/advisor/openai.rs)); the ARW `crs:` key set is the one the writer emits and round-trips ([`src/xmp.rs`](../src/xmp.rs)) |
