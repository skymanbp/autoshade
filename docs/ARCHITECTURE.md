# Autoshop — Architecture

> Status: **implemented** (v0.34.0 — R28, five batches: the X-Trans colour cast
> root-fixed by an in-tree CFA-geometry demosaic, every store read capped
> classwide, AI-mask frame identity in the cache key, per-file 4 GiB memory
> ceilings on BOTH develop doors, and typed XMP read scopes — atop R27's ten
> batches: the input path stopped
> being one camera. **24 RAW extensions + 8 baked formats**
> (`decode::RAW_EXTS` / `pipeline::BAKED_EXTS`, one predicate app-wide), with
> nine cameras — one per format — run end to end from CC0 sample files.
> `batch` and `eval` gained **memory-budgeted `--jobs N` parallelism**
> ([`src/jobs.rs`](../src/jobs.rs)): one 61 MP photo's pipeline pass peaks at
> ~1.77 GB of commit charge, so the worker count is capped by free memory and
> DISCLOSES when the cap overrules the flag — the 147-photo eval went from
> ~2.3 h serial to a measured 38 min at `--jobs 3`. Lightroom **brush and AI
> masks are carried first-class** (dab streams and mask intent round-trip
> byte-exact), and an AI mask's alpha is **recomputed locally** by our own
> segmenter, disclosed as a recomputation in both directions and never passed
> off as Adobe's raster. The style index can additionally rank by a **SigLIP 2
> embedding**. The photographer's own **90° quarter turns** compose into the
> EXIF orientation rather than adding a second rotation stage (§4.4). And the
> radial frame constant `xmp::LR_MASK_FRAME_SCALE` fell from 1.032 to **1.0**:
> the measured 1.032 turned out to be ONE frame's Adobe lens-profile warp
> mistaken for a universal affine, so an imported radial now renders at the
> geometry the sidecar actually stores instead of 3.2 % dilated (§「The radial
> ellipse, measured」). v0.27.0 — R21: deleted version snapshots STAY
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
> 727 library + 11 CLI + 131 GUI + 2+2 contract tests pass in both build
> configurations, with 9 further library tests `#[ignore]`d as forensic probes
> (counts refreshed 2026-08-20 from the v0.34.0 release battery). THREE suites are ADDITIONAL and
> env-gated, so a bare `cargo test` does not include them:
> `AUTOSHOP_LR_PROBE_FIXTURES` (16 real Lightroom radial sidecars, byte
> round-trip), `AUTOSHOP_MB_FIXTURES` (the 7-file M-B forensic set — 42 of its
> 42 corrections imported, 0 refused) and, since R27, `AUTOSHOP_RAW_ZOO` (the
> CC0 nine-camera zoo, one RAW per format —
> `every_make_in_the_raw_zoo_decodes_and_agrees_with_itself` in
> [`src/decode.rs`](../src/decode.rs), 9/9 at v0.34.0). Every release runs all
> three and records their counts — see ROADMAP「发版链 + 环境门套件」.
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
> **Widened in R27 (2026-08-19).** The RAW scope is no longer one make: 24
> extensions, every rawler 0.7.2 decoder with a filename (see
> `decode::RAW_EXTS` — `.x3f` is permanently excluded because rawler's
> metadata reader for it is a `todo!()`). Nine cameras — one per format — have
> now actually been run end to end from CC0 sample files: Canon CR2 + CR3,
> Nikon NEF, Fuji RAF, Olympus ORF, Panasonic RW2, Pentax PEF, Ricoh DNG and
> Sony ARW, where until that batch no non-Sony file ever had been. **The DNG
> on-ramp is the stated answer for anything else**: rawler builds a DNG's whole
> camera profile from the file's own tags (`decoders/dng.rs:270-289`), so an
> Adobe DNG Converter output works for any body without a camera-database
> entry. That sentence lives in exactly one place in the code,
> `decode::DNG_ONRAMP`, so the CLI, the GUI toast and the web error body cannot
> offer three different remedies.
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
>
> ### The ML sidecar family (R27 Batch-5)
>
> There are now **three** Python sidecars, and they share one discipline rather
> than three copies of it. `python/denoise.py` owns the download-and-refuse
> implementation — `_download` with an in-stream byte cap, `_sha256`,
> `_reclaim_stale_parts`, `_fetch_verified` — and the other two **import it**
> (`from denoise import _fetch_verified`) instead of reimplementing it, which
> is why their progress lines announce themselves as `[denoise]`.
>
> | sidecar | bridge | model(s) | licence | size |
> |---|---|---|---|---|
> | `denoise.py` | `denoise.rs` | SCUNet ×5 | Apache-2.0 (KAIR) | ~72 MB each |
> | `segment.py --target subject` | `segment.rs` | U²-Net via a NAMED rembg session | Apache-2.0 | small |
> | `segment.py --target sky` | `segment.rs` | OneFormer ADE20K Swin-L | MIT | ~880 MB |
> | `segment.py --target object` | `segment.rs` | **SAM 2.1 Hiera-Large**, point-prompted | Apache-2.0 | 897,897,416 B |
> | `embed.py` | `embed.rs` | **SigLIP 2 base/16 @384**, 768-dim | Apache-2.0 | 1,501,968,264 B |
>
> **Licence is a selection criterion, not a footnote.** This is a public
> repository whose product is being copyright registered, and a licence that
> restricts *use* is not cured by not redistributing the weights. SegFormer was
> removed in Batch-4 for exactly that (「for research or evaluation purposes
> only」); CLIP and OpenCLIP were passed over in Batch-5 because their model
> cards say deployment is out of scope and the OpenAI HF mirror carries no
> licence tag at all. In both cases the licence-clean option was also the
> stronger model.
>
> **Pinning has two tiers, and the difference is stated rather than smoothed
> over.** `denoise.py`, `embed.py` and `segment.py`'s SAM path fetch every file
> themselves, gate it on sha256 + an exact byte count, and load from a local
> directory with `local_files_only=True` — the digest is the only door.
> `segment.py`'s subject/sky paths still pin only an HF *revision*, which fixes
> WHICH tree is fetched but not the BYTES; that gap is registered in their own
> comments. `trust_remote_code` is never used anywhere: it downloads and
> executes upstream Python through HF's cache, which our gate never sees.
>
> **The sidecars' budget is not the host's budget (v0.34.0).** Every resource
> bound in this tree is shaped like main memory — `decode::MAX_CONCURRENT_DECODES`
> counts 181 MB previews, `jobs` divides `GlobalMemoryStatusEx` — and a model
> loaded into a GPU is in none of them. So the embedder was fanning out
> unchecked: up to four index workers each spawned their own sidecar, four
> resident SigLIP copies at 1.50 GB of fp32 weights apiece, against consumer
> cards that commonly have 4 GB total. Two rules, both in
> [`src/embed.rs`](../src/embed.rs): the sidecar is asked for `--fp16` (which it
> has implemented since R27 and nothing ever passed — CUDA-only by its own
> construction, and it re-casts to fp32 before normalising, so no invariant the
> Rust side checks moves), and calls are SINGLE-FLIGHTED behind a process-wide
> gate, so at most one model is resident whatever the caller's concurrency.
> 4 × 1.50 GB becomes 1 × 0.75 GB. The gate was chosen over wiring `embed.py`'s
> (already implemented) manifest batching because it also covers the
> develop-time query, which no manifest can merge: `batch --jobs 3` is three
> concurrent single-image calls. What it costs is stated rather than hidden —
> the embedding arm of an index build is now serial, while the decode that
> dominates it still runs four-wide, and the decode PERMIT is released before
> the sidecar rather than held across it.
>
> **What the sidecars are FOR** is the part that matters architecturally. The
> segmenter picks *where* — its output is an 8-bit grey raster the deterministic
> engine samples bilinearly — and the embedder picks *which past edits to show
> the advisor*. Neither one decides a slider. And when a sidecar cannot run, the
> feature degrades with a named reason: the style index keeps its 14-dim
> feature, and an AI mask stays carried-but-unrendered with its adjustment
> SKIPPED rather than applied at weight 0 (a zero under `inverted` would apply
> the edit to the whole frame).
>
> **AI masks are a RECOMPUTATION, and every surface says so.** Lightroom's
> `crs:What="Mask/Image"` carries no raster and no geometry — 218 real
> instances, 21 attribute names, longest value 55 characters — only the intent
> (`MaskSubType` + `ReferencePoint` + `MaskName`) and provenance digests. So
> `segment::resolve_ai_masks` runs OUR segmenter at develop time and caches the
> alpha beside the develop, keyed by photo + subtype + click + frame
> (quarter turns, v0.34.0) + backend generation — staged from the render
> source's pixels in the recipe's own frame, so the alpha is segmented in the
> frame it will be sampled in. The result is an approximation of the photographer's intent, never
> a reproduction of Adobe's mask, and `MaskImportReason::AiMaskRecomputed` /
> `MaskLossReason::AiMaskRecomputed` say that in both languages and in both
> directions.

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
Range Mask refinement. Its sliders mirror the global ones plus two that
exist only per mask: a SIGNED `sharpness` (positive sharpens, negative softens
— ACR's local band, R23-1b) and `hue`, a rotation of every colour under the
mask (R23-1b; the global mixer moves one band across the whole frame instead).
`texture` was a third until R25 gave it a global twin — the two now share one
operator and one radius model (`render::texture_pass`, one function since
v0.34.0), which is what lets the engine assert that a full-coverage masked
Texture is bit-identical to the global one at BOTH signs. Since R25 a
mask also carries **four point curves** of its own (`main_curve` /
`red_curve` / `green_curve` / `blue_curve`, LR's bare `crs:MainCurve` … with
no `PV2012` suffix and no space after the comma — deliberately NOT the global
curve's formatter), each independently sparse: 19 of the 160 reference
sidecars use them, mostly one or two channels. Components, `color_gains` and
`role` are **engine-only**; the radial `angle` is rendered, GUI-editable,
AI-settable and — since v0.32.0 — carried in BOTH XMP directions. It is not
`crs:Angle` verbatim: Lightroom's is a tilt in PIXEL space and ours is a
rotation applied in the normalised frame, and the two differ by up to
11.2° of rendered tilt over the ±44° range real sidecars use, so the
boundary folds one into the other (`xmp::lr_to_engine` / `engine_to_lr`,
an SVD of the aspect-corrected ellipse matrix). The fold needs the frame's
ASPECT, which a Lightroom sidecar declares as `tiff:ImageWidth/ImageLength`
and a photo can be asked for; a document that declares none still exports the
unrotated ellipse and discloses the angle it could not write — that is all
`MaskLossReason::Rotation` covers now. The radial's
`crs:Midpoint` and `crs:Version` are read and written back unchanged — carried,
never interpreted. Bitmap rasters are immutable once referenced: every raster
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
| M1 | RAW decode + features (24 formats; Sony ARW only until R27) | **`rawler` 0.7.2** (preview + EXIF + WB; 725 camera models). Missing-preview formats degrade to a neutral develop and say so; third-party parser panics are caught on the CLI as they already were in the GUI | **done** |
| M1 | Baked-source EXIF (ISO/shutter/aperture/focal/date/make/model from JPEG APP1 + TIFF IFD) | rawler's own `Exif::new` + `GenericTiffReader` — no new dependency, and the SAME extraction the RAW arm uses | **done (R27)** |
| M1 | Unified provider framework + GPT advisor + Claude verifier | `ureq` (HTTP) + `claude` CLI | **done** |
| M2 | Deterministic render engine | `image`, custom tone/colour/WB/clarity/NR/sharpen ops | **done** |
| M2 | XMP sidecar writer (ACR `crs:`, global + local masks) | hand-rolled XML | **done** |
| M3 | `auto` end-to-end + batch | batch fixes its work list up front, then runs it through a bounded pool — `--jobs N` since R27, default 3 for `batch` and 1 for `eval` (their pre-R27 concurrency), capped by the memory budget in [`src/jobs.rs`](../src/jobs.rs) and index-ordered on output; since v0.34.0 that budget is per-FILE where the header is free to read (`jobs::survey_peak_mb` — a native-resolution 16-bit TIFF from LR's "Edit in…" can need more than the corpus constant on its own, and says so before the run starts); "pending" = no develop in the store (recipe.json or `<stem>.xmp`, central or legacy) | **done** |
| M4 | Style retrieval + eval harness (your edits as ground truth) | k-NN over EXIF+histogram, plus an optional SigLIP 2 cosine term (`AUTOSHOP_STYLE_EMBED`, off by default); per-field MAE/bias | **done** |
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
as-shot WB). Baked sources skip the sensor path and load via the `image` crate —
with their OWN EXIF since R27, read through rawler's `Exif::new` over the JPEG
APP1 / TIFF IFD block, so the same extraction serves both arms.

**Two lists, one definition each (R27).** `decode::RAW_EXTS` (24) and
`pipeline::BAKED_EXTS` (8) are the only extension lists in the tree. Every gate
derives from them — `is_raw`, `is_source`, `is_baked`, the CLI scanners, the
web server's upload gate, the GUI file dialog. The one copy that cannot be
derived, the static `<input accept>` in `src/web/index.html`, is pinned by a test
that prints the exact replacement string when it drifts. Before this there were
four hand-typed copies and the web one had already lost `.orf`, `.rw2` and
`.raw`.

**What the input path refuses, and what it merely discloses.** The dividing
line is whether the NUMBERS are wrong or only the fine detail:

| Situation | Response | Why |
|---|---|---|
| Unknown make / unknown model / no decoder | **Refuse**, each named separately, each offering `DNG_ONRAMP` | Three different failures wanting three different actions |
| Monochrome or 4-colour (CYGM/RGBE) sensor | **Refuse**, before the develop (`render::refuse_unsupported_sensor`) | The engine emits three-channel colour only; deciding after the demosaic spent a full-frame buffer to learn what the metadata already said |
| A camera RAW named `.tif` | **Refuse**, naming the marker that gave it away | The `image` crate would decode a DNG's first IFD — the *thumbnail* — so the photo would open, look right, and develop at a few hundred pixels. Opening wrong is worse than not opening |
| Third-party parser panics | **Named error**, process survives (`decode::guard_parser_panic`) | The GUI has wrapped workers in `catch_unwind` since v0.22; the CLI had nothing, so one malformed file killed a whole `batch` run |
| No embedded preview (ORF class) | **Degrade** to a neutral develop + say so | rawler overrides no rendition method for 12 of the 24 formats; that is a fact about the format, not a broken file. `embedded_preview` keeps the strict "camera pixels or nothing" contract, because the base-look estimator's method depends on it |
| Non-Bayer CFA (X-Trans) | **Demosaic in-tree over the array's own geometry** (v0.34.0, `render::demosaic_over_cfa_geometry`) + disclose per render | rawler's `PPGDemosaic` is Bayer-only and its guard (`CFA::is_rgb`) checks the pattern's NAME, not its geometry; through v0.33.0 its chroma pass left R unwritten at 8 of the 36 photosites per tile and B at a different 8 — the measured green-dark cast. Now every channel interpolates only from photosites that measured it: colour/tone/framing correct, fine detail approximate and disclosed |
| A RAW whose develop would peak over **4 GiB** — 138,547,333 px and up, at the measured 31 B/px (a 150 MP back; a 102 MP GFX is well clear) | **Refuse**, naming the estimate and its per-pixel basis (v0.34.0, `decode::refuse_raw_develop_over_ceiling`, charged in `render::render_to_image_in` before the sensor is decompressed) | The baked door has refused an over-ceiling file since L02 while the RAW door had NO per-file limit at all, so a ~150 MP back on the default `batch --jobs 3` was the worse instance of the same defect with nothing opt-in about it. The ceiling is the SAME 4 GiB, and the message says outright that `--jobs 1` is not the answer — a single file's peak is not a concurrency budget. This IS a behaviour change: such a file used to be attempted, and would page |
| DefaultCrop rectangle off the sensor | **Disclose** and develop un-aligned | Was a silent `None` until R27 — any diagnosis of a misplaced mask started with zero telemetry |
| Untagged **16-bit** baked input | **Disclose** | It is read as sRGB. Right for 8-bit JPEG (web convention); often wrong for 16-bit, which is what an editor produces — LR's "Edit in…" exports ProPhoto. Warning on every untagged JPEG would be a warning nobody reads |

**Which way is up comes from EXIF, not from `RawImage` (v0.30.0).** rawler
0.7.2 hard-codes `RawImage.orientation` to `Normal` for every decoder except
DNG and QTK (`rawimage.rs:389` and `:478`, verbatim `orientation:
Orientation::Normal, //cam.orientation, // TODO fixme`), so until v0.30.0 a
portrait ARW was displayed, developed and exported sideways even though the
orientation stage had been at the head of the chain since 55e7e07. The real
value rides in `RawMetadata.exif` (IFD0 tag `0x0112`), and
`decode::raw_orientation_of` is now the single accessor all ~~three~~ **five**
consumers read — the render/export hook in `render_to_image_in`, `decode_raw`'s
display dimensions and preview transpose, `camera_rendition`, `frame_size`
(added v0.32.0, and the one that feeds the LR radial projection's aspect) and
`pipeline::migrate_recipe_coord_frame`'s path-based twin. A missing tag answers
`Normal` where rawler's own `from_tiff` answers `Unknown`; the two are the same
no-op on the pixel, coordinate and dimension chains, which is asserted rather
than assumed (`unknown_and_normal_are_the_same_no_op`).

**…and the photographer's own quarter turns compose with it into ONE
orientation (v0.33 / R27).** `EditRecipe.quarter_turns` (0..3, clockwise) is
what the toolbar's ⭯/⭮ write, and `render::compose_orientation` folds it into
the EXIF state before any consumer sees either. This works because the eight
EXIF states ARE the dihedral group of the square and are closed under
composition: a user turn on top of a `Transpose` file lands on a state
`oriented` / `orient_point` / `orient_recipe_coords` already handle, so the
engine gains **no second rotation stage** — the property
`compose_orientation_is_the_composition_of_the_two_coordinate_maps` checks
exhaustively over all 9 × 4 pairs. `quarter_turns` is a separate field from
`coord_era` on the same argument `coord_era` is separate from `version`, one
step further: `coord_era` is which frame the stored numbers are IN (a storage
epoch, migrated once), `quarter_turns` is what the photographer ASKED FOR (a
live edit, undone as easily as a slider). It is the only recipe field written
`skip_serializing_if` — an absent era stamp means something different from a
present zero, whereas an absent turn means exactly zero, so skipping is
lossless AND keeps an un-rotated recipe byte-identical to what v0.32 wrote (R21's
structure fingerprint therefore needs no re-archive pass, unlike v0.31.0's).
`pipeline::rotate_recipe` is the one mover: geometry through
`orient_recipe_coords` **by the delta, never the running total**, raster masks
really turned (`image::rotate90` is lossless and these are our own PNGs — the
`coord_era` migration could only disclose them because those files predated a
frame nobody could re-derive) into freshly claimed names with the originals left
in place, and the turn count last; a raster that cannot be turned refuses the
whole operation rather than leaving a half-turned develop. The XMP sidecar
turns with it since R27 Batch-3 (A8): a document THIS build writes declares its
frame — `tiff:ImageWidth/ImageLength` in the SOURCE frame plus a
`tiff:Orientation` carrying the COMPOSED state (`xmp::frame_declaration`, fed
by `pipeline::photo_frame_aspect`) — which closed the 「the sidecar describes
the unturned frame」 gap Batch-2 registered. The declaration is added only where
the merge target mentions no `tiff:` at all, so the 16 real Lightroom sidecars
still round-trip byte for byte; and the turn is still not RECOVERABLE from a
classic ACR sidecar on the way back, so `recipe.json` remains the authoritative
restore path. Not turned: a photo carrying BAKED pixels — a retouch/AI master
is a raster on disk in the frame it was baked in, so the GUI refuses the turn
with that reason on the button rather than turning the canvas out from under
it.

**And WHERE the frame starts comes from the DefaultCrop rectangle, not from the
sensor corner (v0.32.0).** Block registration of eight Autoshop renders against
their Lightroom exports put every one of them **(+31 ± 6, +20 ± 1)**
full-resolution pixels off, a pure translation with no scale component. The
ARWs carry `DefaultCropOrigin = (32, 20)`, `DefaultCropSize = (9504, 6336)`
inside a `9600 × 6376` raw frame: Autoshop emitted the right SIZE from the
wrong ORIGIN, so recipe coordinates and Lightroom coordinates disagreed by
0.34 % of the width at the frame edge. Two facts in the dependency compose to
produce it, neither wrong on its own — rawler builds the ARW's `active_area`
from the `SonyRawImageSize` tag, which carries a SIZE, so the origin is pinned
at `(0, 0)` (`decoders/arw.rs:707`); and its `CropDefault` step applies the
default crop only `if crop.d != intermediate.dim()` (`imgop/develop.rs:216`),
which a pure TRANSLATION never satisfies. `decode::align_default_crop` moves
the demosaic ROI onto the declared rectangle instead, which costs nothing (the
buffer is the same size, read from the right place) where a post-hoc crop would
pay a second full-frame copy. Narrow by construction: it fires only for a
same-size, different-origin pair, refuses a rectangle that would run off the
sensor, and leaves every size-reducing crop to rawler's own step. **Every Sony
ARW render therefore shifts by (32, 20) from v0.32.0 on** — stored crops and
mask coordinates now mean what Lightroom means by them, and a render made
before the fix is 32 px right and 20 px down of one made after.

Because the frame finally turns, **recipes saved before v0.30.0 hold their crop
and mask coordinates in the SENSOR frame**. `EditRecipe.coord_era` records
which frame a recipe's geometry is drawn in (0 = sensor, 1 = display), and
`pipeline::migrate_recipe_coord_frame` turns an era-0 recipe exactly once at
load through `render::orient_point` — the coordinate twin of the pixel
transform, a bijection per orientation state, so the migration is lossless and
reversible. It is deliberately a NEW field rather than a `version` bump:
`version` is the base curve's provenance and is *transplanted* between recipes
on purpose (paste, the Analyze writer, `photo_calibration`, the quit-time
re-stamp), so folding a coordinate frame into it would let a target photo's
era-2 stamp land on geometry that is already display-frame and get it turned a
second time. The migration hooks only the paths that read a recipe FILE (GUI
open, the variant strip, version snapshots, batch export, `api_recipe`, CLI
`apply`); recipes arriving from the browser or from the model are stamped
current-frame at their boundary instead. Raster (painted / AI-segmented) masks
are image files, not coordinates: they are left alone and the user is told so.
Imported BRUSH groups join them on that side of the line for a different reason
— their dab coordinates are carried verbatim so the sidecar round-trips
byte-faithfully, and rewriting every dab would forfeit exactly that in order to
migrate a geometry nothing renders yet (`render::recipe_has_raster_masks` is the
predicate for both).
v0.31.0 adds a second stamp built field-for-field on this precedent —
`schema_era` (0 = written before the R25 control set existed) — for the same
class of reason: see 「the merge treats ignorance as ignorance」 in §4.5.

The migration's crop arm has one subtlety worth naming, because a reader will
otherwise look for the hole R24 registered and not find it (R27 L-16c). A crop
rectangle is normalised against the STRAIGHTENED frame — `render_pipeline`
straightens before it crops — not against the sensor rectangle. That turns out
to cost nothing for a quarter or half turn: `render::inscribed_dims` is exactly
swap-equivariant in `w`/`h`, and rotations commute with each other, so the
inscribed frame of the turned photo IS the turn of the inscribed frame and
`orient_point` maps into it with no residue. What the registration was really
pointing at is a SIGN: `rot(θ) ∘ mirror == mirror ∘ rot(−θ)`, so under the four
mirroring orientation states the straighten was being applied backwards by
`2·θ`, and every crop coordinate then indexed content that had rotated out from
under it. `orient_recipe_coords` now negates `straighten_deg` on exactly those
four states — the same rule it already applied to an ellipse's `angle` — and
all eight states are exact.

**Lightroom's crop is the same rotated-corner encoding the mask box uses**
(R27 Batch-3, `P3-cropangle-model.md`). `crs:Crop{Left,Top,Right,Bottom}` are
not an axis-aligned rectangle: they are two opposite ROTATED corners of the
crop rect as fractions of the un-rotated source frame — `X = (R−L)/2·W`,
`Y = (B−T)/2·H` signed, `p = X cosθ + Y sinθ`, `q = −X sinθ + Y cosθ`, exported
size `round(2p) × round(2q)` — which is bit for bit the decode
`xmp::lr_to_engine` runs for `Mask/CircularGradient`. One family, two consumers,
and the ONE difference is measured rather than assumed: there is no `k = 1.032`
magnification on the crop (global best-fit scale 1.000006). The sign is
`rot(source → export) = −CropAngle`, so `straighten_deg = −CropAngle`, and like
`k` that negation lives only at the XMP boundary — the engine's clockwise
convention does not move. `xmp::lr_to_engine_crop` folds Lightroom's
(corners, angle) into this engine's (inscribed-frame rectangle, straighten) so
the render pipeline is untouched; the conversion is exact except where a
Lightroom rectangle pushed against the edge of the rotated frame reaches
outside the CENTRED inscribed rectangle, which is clamped and DISCLOSED
(`xmp::crop_import_note`) rather than silently trimmed by `EditRecipe::clamp` at
the next save. Measured on the seven library specimens the model rests on, that
clamp costs 0, 0.2, 1.2, 1.3, 6.9, 30.3 and 5.9 px.

**Forward compatibility across all four persisted shapes** (R27 L-17 completes
the set). `EditRecipe` and every plain struct in it carry `deny_unknown_fields`,
so a build meeting a recipe from a NEWER build refuses it loudly and by name
rather than reading a truncated version of it. From R27 that is true of the two
internally tagged enums as well: `MaskGeometry` and `RangeMask` now carry the
attribute on the CONTAINER (which covers every variant and exempts the `kind`
tag). Until then they were the one silent hole — v0.31.0's two new `Radial`
fields were dropped without a word by a v0.30 binary, which then wrote the
truncated geometry back, editing the user's file on their behalf. The old
comment claiming serde cannot deny unknown fields on an internally tagged enum
was simply wrong, and was checked against serde 1.0.228 before it was removed.
`variants.json` is the deliberate exception in the other direction (see the
strip's `#[serde(flatten)]` capture-all above): it is a LIST the two builds
share, so tolerating and carrying unknown members is the correct behaviour
there, while a recipe is a complete statement and a partial reading of one is
never safe.

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
orientation → working-resolution cap → lens geometry (distortion + CA) →
straighten → crop. Then the pixel stage, in this order: anchored white balance →
lens-profile vignette → manual vignette → **dehaze in linear light** (before any
tonal work, so the airlight estimate cannot move when Exposure is dragged) →
tone LUT (exposure/contrast/whites/blacks/highlights/shadows, the tone curve and
the per-photo camera base curve composed into one table) → per-channel RGB
curves → 8-band HSL → colour grading → clarity → **texture** →
saturation/vibrance → noise reduction → sharpening → local adjustments
(linear/radial/bitmap masks).

Two R25 additions ride existing stages rather than adding one. **Texture**
(`render::texture_pass`) is a small-radius detail operator (0.005·min(w,h),
floor 2), placed between clarity and saturation for ACR's Basic-panel order and
called by the global stage and the mask arm alike — so 「Texture +30」 means the
same structure globally and inside a mask, on a 1280 px preview and at 61 MP.
**Its two halves are not the same operator, since v0.34.0.** Positive is the
plain `unsharp_luma` of clarity at that radius, measured against Lightroom in
R27. Negative is **band-limited**: it subtracts `blur(r/4) − blur(blur(r/4), r)`
rather than the full high-pass, so the transfer returns to 1 at both ends of the
spectrum and dips only in the middle (the coarse plane is the fine one blurred
further, so the band is bounded by the fine blur's own response). The old negative half ran the same
unsharp with a negative amount, whose endpoint is exactly the blur — 「Texture
−100」 was a full Gaussian blur that erased edges and fine detail, which the
visual-inspection package measured (σ −92 %) and Lightroom's mid-band control
does not do. **This IS a rendering change for every negative texture value,
global and per mask.** There is no Lightroom ground truth in this repository for
the negative transfer curve, so the fine-radius fraction and the notch depth are
OURS, chosen to keep the endpoint band-limited, not fitted to Adobe — the same
stance `manual_vignette_lut` takes, and the XMP still carries the raw slider so
Lightroom re-renders it with its own model. What is pinned by test is the SHAPE
(`texture_at_minus_100_is_band_limited_not_a_full_blur`: at −100 a 4 px tone
keeps 96 % of its contrast, where the old branch left 0.1 %, while a 16 px tone
still loses 71 %). **Manual CA** (`ca_r`/`ca_b`) folds into the
per-channel radial factor the lens-profile CA correction already builds
(`MANUAL_CA_PER_UNIT = 2e-5` per slider unit, i.e. ±0.2 % of the half-diagonal
at the ends — derived beside the constant), so it is a scaling of an existing
LUT, not a new operator; every geometry consumer reads the one
`geometry_profile` funnel so preview, canvas, export and the web surface can
never disagree about whether the frame moved.

Each mask runs its own sub-chain, in this order: **dehaze** → the fused
**WB + tone + curves + saturation (+ local hue)** blend → **clarity** →
**texture** → **sharpness** (signed ±100, the global sharpening stage's own
radius model — R23-1b) → **noise reduction**. The mask's own **point curves** (R25) live inside that fused pass
and cost no extra one: `main_curve` is handed to the same `build_tone_lut` that
already composes the mask's synthesized tone recipe, and the three RGB curves
are compiled once per mask and applied right after it — the global chain's
1 → 1b → 3 order mirrored locally. Splitting them out into a pass of their own
would have changed the output of every existing partial-weight mask, the same
reason R22 gave for not splitting the blend.
Clarity/dehaze/texture became engine-rendered in R22 (feedback
#15a/#10B — until then they were carried, exported to XMP and drawn only by
Lightroom, so a mask that moved only those three did nothing in-app; recipes
saved before R22 re-render with the local effect, which the user signed off on).
Local dehaze shares the global haze model, with the airlight estimated once per
frame so mask order cannot change it; clarity is a mask-weighted unsharp at a
large midtone-masked radius, and texture is `texture_pass` at a small one —
the same function the global stage calls, negative branch included. Two deliberate residues vs the global order are documented at
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

Since v0.13.0 Autoshop does **not** write it next to the photo: the source
library is read-only, so the projection lands in the per-user develop store
(`<AUTOSHOP_DATA_DIR | %LOCALAPPDATA%/autoshop | $XDG_DATA_HOME/autoshop |
$HOME/.local/share/autoshop>/develops/<stem>-<hash of the absolute
path>/<stem>.xmp` — see `store::store_root_with_trust`, and the trust bullet in
§3 for why the shared-temp last resort is labelled rather than
trusted), alongside `recipe.json` (the authoritative develop
state), version snapshots (a deleted snapshot registers its number + content
fingerprints in the develop's permanent `.deleted-versions.json`: the number
is never re-issued and the backup gate stops auto-preserving the discarded
content — a discard record, not a recovery copy), version NAMES + provenance
(v0.30.0: `.version-meta.json`, an ADVISORY sidecar recording per number the
user's own name plus which variant the snapshot came from and whether it was
taken explicitly or by the backup gate; it follows the same non-generational
discipline as `.deleted-versions.json` — no `.bak`, no commit membership, not
swept by a develop clear, carried by adoption — and a name dies with its
number, which is never re-issued), mask rasters, `pixels.json`
(the baked pixel-master
link) and, since v0.22.0, `variants.json` — the GUI's variant strip
(background variants' kind/recipe/raster origin + the active card's
three-valued kind, each card since v0.30.0 also carrying an opaque stable
`id` and an optional `name`, both additive at `v=1` in BOTH directions: the
record is not `deny_unknown_fields`, so an older build reads a newer strip
without refusing it, and a `#[serde(flatten)]` capture-all on `VariantsRecord`
/ `VariantEntry` carries members it does not know back out again — which is
what makes the read-modify-write arm of `store::variants_member`
(`ActiveWrite::Kind`, used by the CLI `match` and the reverse-fit worker) safe
to run against a strip a newer build wrote. Read tolerance alone would have
had that arm publish a truncated record over the newer file. A writer that
OWNS the whole strip (`ActiveWrite::Strip`, the GUI) authors the record from
its live cards and replaces it wholesale, by contract; an empty capture-all
serialises to nothing, so files that use no future member are byte-identical
to what earlier builds wrote), which is what lets a 「反推 Reverse-fit」 or 「AI 生成」
card survive a reopen and lets the quit dialog's Save-all genuinely save
background variants instead of livelocking.

The strip record's ACTIVE half (`active_kind` / `active_pos` / `active_id` /
`active_name`) describes the card whose develop `recipe.json` mirrors, so
v0.30.0 gives every writer of a develop ONE way to keep it truthful:
`store::variants_member(src, ActiveWrite)` produces the commit's `variants`
member and nothing else decides it. The GUI (Ctrl+S, the Analyze landing)
OWNS the strip and hands the whole record over — `Strip(Some(rec))`, or
`Strip(None)` when the strip went trivial, which clears the file; a writer
that only knows the KIND of develop it published says so (`Kind("fitted")` —
the CLI `match`, the GUI reverse-fit worker; never `"original"`, because a
photo has exactly one base negative and a second Original card could never be
deleted again); a writer that learned nothing about the card leaves the record
standing (`Unknown` — the web save, the batch paste). Before this, five
production writers published a new active develop with `variants: Keep` and a
CLI `match` over a photo whose record said `original` reopened as 「▣ 原片」
holding a reverse-fit; the batch paste was the fifth and did not go through
`commit_develop` at all (it now does, so its recipe write is a single
generation like every other). A "trivial" strip is one card, kind Original,
with no name and no minted identity (`model::strip_is_trivial`, shared by the
live strip and the navigation stash) — since the cards became renameable in
v0.30.0, a name or an id IS a reason for the record to exist, because nothing
else stores either.

On the strip itself the ACTIVE card carries the actions that act on the live
canvas: 「＋」 saves the develop as a numbered snapshot (v0.30.0), the card's
name is editable in place (the rename buffer is keyed by the card's own id, so
an async push that renumbers the strip cannot land the text on another card),
and 「▣」 copies this card's develop onto the ▣ Original card. That last one
keeps two things apart on purpose: it overwrites the Original CARD's develop
PARAMETERS — its baked pixels and raster origin stay, and the source card
survives (Lightroom's 「Set Copy as Original」) — while Ctrl+S is what
afterwards makes that card's develop the photo's saved develop in
`recipe.json`. It is one Ctrl+Z, because it lands the canvas on the Original
first (which reseeds history with that card's own develop) and commits exactly
one step; a pixel-state card shows the button DISABLED with the reverse-fit
remedy, the same judgement Ctrl+S's XMP refusal and 「＋」 apply. The 「✕」
ARMS before it fires: deleting a card is irreversible in a way deleting a
version is not the mirror of — undo history is per-variant by contract and no
registry can bring a card back the way `.deleted-versions.json` keeps a
deleted number burned — so the first click asks and the second deletes.

Those three buttons are drawn by ONE owner (`variant_card_buttons`) and their
clicks are performed by one more (`dispatch_variant_action`, taking a single
`VariantAction` so the arms stay mutually exclusive by construction rather than
by an `else if` chain's shape). The Versions section's edit-state list draws the
same buttons through that owner and switches cards on a row click, so a card
action means one thing on both surfaces; renaming stays on the strip alone,
because two `TextEdit`s over one buffer fight for the caret every frame. The
list's buttons take a second, indented line rather than joining the label row —
a 320 px panel already holds a kind label, a name and a vocabulary note, and an
unbounded row is how this side panel crept +8 px a frame twice before.

`store::list_edits(src)` is the read side: a photo's whole edit-state list in
one query — the strip's cards in strip order, then its version snapshots
ascending with their advisory metadata attached (restricted to numbers that
are actually listed, so a kill between a delete's sweep and its metadata drop
can never surface a phantom row). It is a synthesized VIEW, never a stored
one: the two halves live in different files under deliberately different
lifecycle rules (`variants.json` is a generation member a develop clear
sweeps; the `v<n>` family is kept). A live editor's in-memory strip outranks
the variant half, so the GUI consumes the version half and keeps rendering its
own cards; for every non-GUI surface this IS the list. Copy the XMP beside the RAW when
you want Lightroom to pick it up. A Lightroom sidecar that already sits beside
the RAW is READ on open — the newer intent wins — and never overwritten.

**Import (R25).** Until v0.31.0 that read imported *no* Lightroom mask at all.
Two gates each dropped a whole correction on sight: the presence of `crs:Angle`
(LR writes it on every radial, `"0"` included) and a `crs:MaskBlendMode` on a
file we did not author (all 1048 mask components in the 160-sidecar reference
library carry one). Both were the same root as five more 「don't recognise it →
discard the correction, keep an integer count」 gates, while the EXPORT side had
had a named `MaskLoss{name, reason}` since R22 — the asymmetry WAS the defect.
The import side now mirrors it: `MaskImportReason` names eleven LOSSY-but-
imported cases (rotation / blend mode / inert local slider / unknown local key /
extra shapes / brush carried-not-rendered / AI mask recomputed / AI mask
unresolved / foreign range mask / unreadable local curve / curve-refine
saturation) — the geometry still arrives — and two
DROPS kept deliberately distinct from them, `Unrepresentable` (no parametric geometry to stand on) and
`OutOfModel` (values that read fine but land outside the model), so a sentence
saying 「imported with N features unmodelled」 cannot be told about a correction
that was not imported at all. The banner names what it could not model instead
of counting it. Measured on the reference library: 0 masks imported before, 31 of
its 42 corrections after, with `imported + refused == corrections` holding file
by file. (Re-measured through the `AUTOSHOP_MB_FIXTURES` probe as the round went
on: **33 of 42** once v0.31.2's multi-component base-geometry fix stopped a
Correction whose base shape sat behind a Subtract component from being read as
the subtracted shape, then **42 of 42 with 0 refused** at the v0.33.0 release
battery — the nine that were still refused were all `Mask/Image`, which R27
Batch-5's recomputation arm took. The per-photo refusal rate over the 147-pair
eval corpus fell the same way, 2.35 → 0.05 masks per photo.)

**R27 Batch-4 (L-08) took the brush half of the remaining refusal.** A
`Mask/Aggregate` and its `Mask/Paint` children are now a first-class geometry,
`MaskGeometry::Brush` — the group's `(MaskBlendMode, MaskValue, MaskInverted)`
and each stroke's `Radius`/`Flow`/`CenterWeight`/`MaskSyncID` plus its `crs:Dabs`
token stream, the stream carried VERBATIM as a string. It is parsed, kept in
`recipe.json`, and written back into the sidecar as the same `Mask/Aggregate`
element it arrived as. **And it is RENDERED, since R29 Batch-6b** — from a
measured model rather than from Adobe's code. The one input a rasterizer needs
that no sidecar contains is the alpha kernel (the falloff versus hardness and
the per-dab accumulation law — the file stores the STROKE, never the alpha), so
it was MEASURED: 29 controlled Lightroom exports gave
`k(ρ;h) = (1 − ρ^m(h))^n(h)` with `ln m`/`ln n` cubic in the hardness (held-out
rms 0.0109), a one-parameter flow odds law `D(f) = κf/(1−f+κf)` at κ = 0.1284,
SCREEN accumulation, and the stroke's `MaskValue` scaling each dab before the
screen. `render::brush_raster` stamps the dab stream into an 8-bit grey alpha at
develop time (a render-time artefact — no schema field, no `schema_era` gate)
and `render::mask_weight` samples it exactly as it samples a `Bitmap` mask.
⚠ **Render-behaviour hard change:** from R27 Batch-4 until then that arm was the
literal `=> 0.0`, so every recipe holding a brush mask renders differently now.
Both disclosure channels moved with it and are named
`MaskImportReason::BrushRendered` / `MaskLossReason::BrushRendered`
(「brush mask(s) drawn from Autoshop's measured model of Lightroom's brush -
not Adobe's own rasteriser」), because the edges are ours and not Adobe's — the
same shape of statement the AI-mask arm makes, one notch weaker because ours
came from a measurement of Adobe's own output. Measured on the specimen
folder that has brush work in it: `_DSC9583` went from 8 of 11 corrections
imported to 10 of 11, and the one still refused is refused for
`CorrectionAmount="1.1"`, not for its brush.

**The measurement then LANDED, and the arm still does not render (R27
Batches 8-10).** Two controlled Lightroom experiments — 16 hand-made exports,
then 17 states written as SYNTHESIZED sidecars that Lightroom imported and
rendered without complaint — close the alpha model except at one axis:
accumulation is **screen**, within a stroke and across components (a held-out
51-dab stroke predicts at rms 0.0045); **density (`MaskValue`) scales each dab
BEFORE the screen** rather than capping it (the cap reading is refuted 13×);
and **flow** obeys a one-parameter odds law, `D/(1−D) = κ·f/(1−f)` with
**κ = 0.1219 ± 0.0027** (fit rms 0.0070 over 11 rungs, held-out 0.0097, and
`D(1) = 1` exactly with no free parameter — which killed the earlier
「flow 1.0 takes a different code path」 hypothesis). Two identities fell out:
Lightroom's brush **Size is the α = 0.5 diameter** (266.1 ± 5.0 px, invariant
across the feather ladder) while `crs:Radius` is the OUTER support, and
`CenterWeight ≠ 1 − Feather/100` (Feather 50 → 0.1621).

Two named reasons keep the renderer at weight 0. **(1) The kernel has no closed
form.** `k(ρ;h)` is measured at 11 hardness rungs; six families were tried and
the only one spanning h = 0 → 1 has parameters DISCONTINUOUS in h, so what
exists is a measured TABLE whose h-interpolation predicts a held-out rung at
rms 0.0115 and max **0.0344** — 4× the 0.0085 quantisation floor. **(2) The
mask does not live in the frame this engine renders in**: Lightroom rasterises
it BEFORE its lens correction (the same artefact the `k = 1.032` bullet below
closes), displacing exported dabs by up to 57 px and stretching them 7.4 %
anisotropically at the frame corners — and this engine has no `.lcp` parser,
never reads `crs:LensProfileEnable`, and runs Sony EXIF knots in its own
geometry stage, a DIFFERENT polynomial. Baking a mask into pixels at a position
known to be wrong is worse than the honest `BrushCarried` disclosure, so the
implementation waits: the sketch is on file (`batch10-report.md` §7.4 —
pre-rasterise the dab group and sample it exactly like `Bitmap`/`AiMask`, no
schema change, with κ and the 11-rung table as the pinned test values), and the
frame half of the blocker is what an `.lcp` reader would answer (the named R28
candidate).

Reading a Paint required three parser fixes in the same batch, all of them
latent-until-now: `classify_correction` walked the component list FLAT (so a
stroke inside a group read as a sibling of it), `base_geometry_at` searched the
whole correction segment nesting-blind (so a gradient nested in a group could
have been promoted to the correction's base shape), and `parse_one_correction`
read its geometry keys from a slice running to the END of the correction (so a
later component's `MaskValue` could answer for a base that omitted its own).
All three are nesting-aware now, through one shared `components_in` walk.

`Mask/Image` — the AI subject/sky/object masks — was the other half, and R27
Batch-5 took it. Those files carry no raster and no geometry payload at all:
only the INTENT (`MaskSubType`, `MaskName`, `ReferencePoint`), the provenance
digests, and the proxy geometry the model ran in. So reproducing one was never
a parser question: it needs a segmenter of our own producing our own alpha,
which is a DIFFERENT feature — `MaskGeometry::AiMask` carries the intent and
the 11 whitelisted provenance attributes verbatim, `segment::resolve_ai_masks`
recomputes the alpha at develop time and caches it, and every surface calls it
a re-derivation (see 「AI masks are a RECOMPUTATION」 above). With that arm the
7-file M-B set imports 42 of 42 corrections and refuses none.

**Every front-end hears it, the CLI included (R27).** `xmp::import_losses` →
`xmp::describe_import_losses` is the one producer; the GUI localises it into the
open banner (`bin/gui/export.rs`, `bin/gui/persist.rs`), and the CLI prints the
English sentence on stderr beside its `xmp -> …` line
(`main::lightroom_import_note`, from `analyze` / `auto` / `match`). Until R27
that producer had GUI callers only, so a headless run over a photo whose sidecar
held a brush mask said nothing at all about it. `batch` is deliberately silent:
its work list is filtered by `store::has_develop_or_sidecar`, so a photo that has
a Lightroom sidecar never enters it. `eval.rs` stays narrower on purpose — it
counts `unsupported_corrections`, i.e. DROPS only, because its ruler needs
「imported + refused」 to equal the size of the user's local work.

**And the merge treats ignorance as ignorance, not as an instruction** — the
one law behind the round-end fix. A recipe holding a default for something it
has never read is not a photographer clearing it: a recipe with no masks does
not delete the sidecar's mask block (the test is 「does the BASE have one」, not
「was the import lossy」), an era-0 recipe (`schema_era`, `coord_era`'s twin,
0 = written before v0.31.0) neither strips nor emits any of the 27 R25 scalar
keys still sitting at their untouched default, and the pass-through map's
absence is likewise not a clear. All three are per KEY, not per file: drag one
of those sliders on a legacy photo and it writes, because THAT is a statement.
The accepted cost is stated where the rule lives — deleting every mask inside
Autoshop no longer propagates the deletion to the sidecar (delete them on the
Lightroom side), and republishing a mask block you HAVE edited recasts
`MaskName`/`MaskSyncID` deterministically from our writer. The user re-examined
both on 2026-08-19 and confirmed them as settled (ROADMAP L-19), so they are a
documented trade, not an open question.

The **export** direction is disclosed the same way (M6a). Classic ACR XMP
cannot express everything the engine renders, so the writer names what it left
behind while it emits: raster (bitmap) and muted masks are skipped whole, extra
Add/Subtract/Intersect shapes flatten to the base geometry, a rotated radial
exports unrotated **when and only when the document declares no frame** (v0.32.0
narrowed it from every rotated radial; the note says by how many degrees and
why), and per-channel recolour gains do not travel. The verdicts are
the writer's own, produced by the ONE loop that emits the mask block (so the claim
cannot drift from the file) and handed back with the document itself —
`xmp::recipe_to_xmp_with_losses` / `MergeOutcome::losses`, one pass per save;
`xmp::mask_export_losses` is the same list for a surface that only wants the
disclosure and writes nothing. It rides `pipeline::write_xmp`'s return value to
every surface: the GUI localises it into the save status line and a toast, the
web reply carries it in the note it already had, and stderr gets one line from
`write_xmp_doc` for the CLI. `recipe.json` remains the lossless sidecar, which
is what the disclosure says. Since v0.30.0 the GUI's line NAMES the masks
rather than counting them (the CLI's has since M6a; 「which of my twelve?」 was
the half a count could not answer).

#### The radial ellipse, measured (v0.32.0)

Both XMP directions used to read `crs:Top/Left/Bottom/Right` as a bounding box.
**It is not one.** It is the pair of ROTATED CORNERS of the ellipse's own box,
written in the frame's PIXEL coordinates, so the decode is

```
X = (R−L)/2·W          Y = (B−T)/2·H          SIGNED, never abs()
a =  X·cos θ + Y·sin θ    b = −X·sin θ + Y·cos θ      guard: a > 0 ∧ b > 0
```

with `θ = crs:Angle`. The naive reading gets the axis RATIO wrong by a median
factor of 1.84 over the user's rotated components (p90 4.86, max 40.7), often
assigns the major axis to the wrong axis, and leaves 16 of 195 components
literally unreadable (a negative semi-axis). The corner model is not a fit: with
no free parameters it predicts that `Left > Right` forces `Angle > 0` and
`Top > Bottom` forces `Angle < 0` and that both at once is impossible — the
library agrees 16/16, p = 2.5 × 10⁻⁵ — and the two rendered subjects land on
it at the pixel (`_DSC9689` 8.3 : 1 at +24.35°; `_DSC9685` decoded tilt
−60.486° against a measured −60.5°).

Three more constants ride with it, each measured on the same twelve-export
experiment:

* ~~**the frame affine `k = 1.032`**~~ — real as a MEASUREMENT, wrong as a
  CONSTANT; `xmp::LR_MASK_FRAME_SCALE` is **`1.0`** since 2026-08-19 (R27
  Batches 8+10, user ruling). The measurement stands as history: on the probe
  frames Lightroom's mask does sit in a frame 1.032× the export and concentric
  with it, `x_px = W·(k·n − (k−1)/2)`, and on a hard-edged mask whose centre
  sits 2799 px from the frame centre that affine lands to **3 px** where "the
  centre stays put" misses by 88 — the CENTRE moves, not only the axes. What
  it is not is universal: three further controlled frames measure the same
  scale at 0.984 (24 mm), 0.9996 (105 mm) and 1.004 (34 mm), i.e. it is a
  PER-FRAME quantity. The mechanism is **Adobe's lens-profile distortion**.
  Toggling `LensProfileEnable` 1 → 0 on the identical capture and radial moves
  the implied scale 0.98396 → 0.99826, and independently 11 disjoint brush dabs
  are displaced PURELY RADIALLY by `dr = −0.02487·r + 2.285e−9·r³` (rms
  2.94 px; a pure scale refuted at 11×, this constant at 30×). Over one mask's
  narrow annulus a distortion polynomial is locally indistinguishable from a
  scale, which is exactly why single frames read 0.984 … 1.032 at all. So the
  sidecar's geometry lives in the PLAIN frame (0.998 measured with the profile
  off) and **Lightroom rasterises its masks BEFORE the lens correction**, a
  frame this engine cannot reproduce today — no `.lcp` parser,
  `crs:LensProfileEnable` never read, and its own geometry stage runs Sony EXIF
  knots, a different polynomial. Rendering the geometry the sidecar actually
  stores and leaving Adobe's warp UNMODELLED is what the user ruled; an `.lcp`
  reader is the named R28 candidate. The `k` plumbing stays (it is the shape a
  warp model slots into) and is now the identity everywhere. **What this
  costs and what it does not:** the byte round-trips were `k`-invariant by
  construction, so the real-sidecar suites pass unchanged; what moves is the
  RENDER — an imported radial is no longer dilated 3.2 %, and the residual on
  any frame is that frame's own Adobe warp (0–3.4 % observed).
* **the falloff endpoints** — measured in v0.32.0, REFUTED at both ends a round
  later, and deliberately still in the code. What v0.32.0 landed, and what the
  engine runs today, is `ramp(1 − f, 1 + f/2, d)`
  ([`src/render.rs`](../src/render.rs), the `MaskGeometry::Radial` arm of
  `mask_weight`): a cubic smoothstep — the family the engine already used —
  from `d_in = 1−f` to `d_out = 1+f/2`, fitted on an 11-rung exposure ladder
  across five frames (aspect 1.03 … 7.46, one rotated, one corner-placed). The
  engine's outer edge had been at `d = 1`; the measured one at `Feather = 50`
  is 1.25, i.e. **the mask was 29 % under-sized** and no amount of correct
  geometry upstream could recover it. The law was written with a `k` on both
  endpoints, folded into the semi-axes at the XMP boundary; at
  `LR_MASK_FRAME_SCALE = 1.0` that factor is the identity and folds out, so
  imported and engine-native radials share one law outright instead of by
  cancellation. **R27 Batch-10 then measured the WHOLE feather range on two
  geometries and refuted both endpoints.** An untouched reference export
  unlocked the inner branch for the first time: `d_in` reads 0.558 / 0.348 /
  0.041 / **−0.144** at `Feather` 25/50/75/100 against the law's
  0.75/0.50/0.25/0.00 (measured `d_in ≈ 0.79 − 0.94 f`, negative at the end
  stop). `d_out` SATURATES at ≈ 1.41 instead of climbing to 1.5 — 1.220 /
  1.409 / 1.419 / 1.433 on a 4.35-aspect off-centre ellipse, reproducing the
  first geometry's 1.223 / 1.402 / 1.406 / 1.414 to ≤ 0.019. The outer branch
  IS a clean smoothstep (rms 0.0006–0.0072, at or below the quantisation
  floor); the inner branch is not the same one (forcing a single smoothstep
  across both costs rms 0.012–0.018). The code is unchanged on purpose: the
  falloff sits on the same reviewer-owned geometry surface as the frame
  constant above, and a two-branch replacement law needs its own adjudication
  (`batch10-report.md` §8; the item lives in V2_PLAN §7 item 1 — M1_PLAN and
  V2_PLAN are development ledgers kept outside the public tree since
  2026-08-20, the same standing as the probe reports these sections cite).
* **`crs:LocalHue`'s scale is 180, not 100.** A controlled export with the mask
  Hue slider at +50 wrote `crs:LocalHue="0.277778"`; 0.277778 × 180 = 50.00004.

~~`W, H` are the exported pixel dimensions (= `DefaultCropSize`)~~ **ERRATUM,
R27 Batch-3:** `W, H` are the **un-rotated SOURCE frame** — `DefaultCropSize`,
which equals the exported dimensions only while `HasCrop="False"` and the
capture is landscape. The two readings diverge the moment a crop exists
(`P5-cropped-mask-frame.md` §1: reading a cropped export's own dimensions
displaces `DSC09401_16.9.JPG`'s five radials by 834–1384 px) and again on a
portrait capture (`P1-portrait-mask-frame.md` §1: an `Orientation=8` export is
already upright and carries no `tiff:` at all, while its `crs:` numbers are
still fractions of the 9504 × 6336 sensor array). Only the RATIO survives the
ellipse algebra, but the boundary now carries the SIZE and the source→display
TURN as well (`xmp::FrameAspect`): the size because the writer declares it
(`tiff:ImageWidth/ImageLength`, so a document we wrote can be read back), and
the turn because the reader moves the whole recipe through
`render::orient_recipe_coords` once it has decoded in the source frame. The whole projection is `xmp::
lr_to_engine` / `engine_to_lr`, an exact algebraic inverse: 16 real Lightroom
radial sidecars round-trip their four corners **byte for byte** (the angle to
10⁻⁴ °, bounded by the `f32` the recipe stores it in), including the rows where
Lightroom itself writes `Left > Right`.

**What this costs.** `recipe.json` is unchanged — no new field, so no older
build refuses a v0.32.0 recipe over its masks — but the RENDER of a stored
radial moves: the falloff's outer edge is wider, so a recipe saved before
v0.32.0 draws a slightly larger, softer radial than it did. That is a
deliberate, disclosed change (version snapshots keep the earlier render), and
re-importing the original Lightroom sidecar is what recovers the intent for a
mask that came from one. (v0.33.0 moves a stored radial a SECOND time, in the
other direction and for the frame constant rather than the falloff — the first
bullet above.)

#### The five-tier control registry (v0.30.0; populated in v0.31.0)

Two independent facts decide what a develop control IS — does the ENGINE render
it, and does it reach the SIDECAR — and until R24 nothing wrote them down
together, so 「the GUI offers a slider the engine ignores」 and 「the save
silently drops what you are looking at」 were both unrepresentable claims.
`advisor::catalogue::Tier` names one (renders x exports) combination each:

| tier | engine renders | own `crs:` property | members today | disclosed by |
|---|---|---|---|---|
| `Rendered` | yes | yes | the ordinary controls — **29 global, 23 local** (R25 added global `texture` and the manual CA pair `ca_r`/`ca_b`; per mask, the four point curves) | — nothing to disclose |
| `CarriedOnly` | no | yes | **25**: the 24 unpublished-operator globals R25 ruled on — the six post-crop vignette keys, the three grain keys, the five Sharpen/Noise detail axes, the three colour-NR keys, `auto_lateral_ca`, the six de-fringe keys — plus the mask `name`, a label rather than an operator. Every one carries its REASON in `CARRIED_ONLY_GLOBAL` / `CARRIED_ONLY_LOCAL` | `xmp::global_render_gaps` → 「carried to Lightroom, not rendered here」, on the control and in the save line |
| `PassThrough` | no | verbatim | **1 row, 16 keys**: `passthrough`, a `BTreeMap<String,String>` over the named `xmp::PASSTHROUGH_CRS` block — the 8 Perspective/Upright keys and `CameraProfile` + the 7 Camera Calibration keys. A NAMED key set, deliberately not 「everything unknown」: the merge's strip universe is a static list, so a free-form map would desynchronise from what is actually written. `unmodelled_global_crs` keeps naming the rest — that is the feature, not the omission | the read-only Transform / Calibration section: values shown, no slider offered, because a slider on something never interpreted would be a lie |
| `RenderedNotExported` | yes | no | `base_curve`, `lens_profile`; per mask `components`, `enabled`, `color_gains` | `xmp::global_export_losses` + the mask loss list |
| `DerivedWriteOnly` | (yes) | no — only a derived value | `as_shot_k`/`as_shot_tint`, which reach the sidecar as `crs:Temperature`/`Tint` | — |

`Control.tier` is `Option<Tier>`; `None` is not "unclassified" but "not a
develop control" (the base curve's `version` stamp, the two era stamps, the
AI's own `rationale`/`confidence`, the solver's mask `role`, and since v0.33.0
the photographer's `quarter_turns` — seven rows), and such a row may never own
a `crs:` property. The registry is enforced from three sides: adding a field to `EditRecipe` /
`LocalAdjustment` already fails the build until it has a row (the `global_value`
/ `local_value` destructures, R23-1), the row cannot be written without a tier
(a struct literal has no optional fields), and the tests re-derive both halves
of the claim — the EXPORT half from `Control::crs`, itself pinned against the
XMP writer's `owned_attr_keys`, and the locals' RENDER half from the renderer's
own activity gate, probed one-hot and by zeroing (which is what tells a
multiplier like `amount` from an inert field).

Two **inclusion laws** ride on it, one per surface, both read off the registry
rather than transcribed from it: a control the AI is ASKED for
(`!engine_only`), and a control the GUI can MUTATE (extracted from the GUI's own
source at test time), must be one the engine renders — or a `CarriedOnly` entry
on an explicit allow-list carrying its reason. A slider that moves a number and
no pixel is the worst kind of bug here: it looks like it works, it survives a
save, it reloads, and the photo never changes.

The registry also generates the three disclosures the mask-side story was
missing. Outbound, `xmp::global_export_losses` names the active
`RenderedNotExported` controls — a photo whose look depends on its camera base
curve or its lens-profile correction used to export a sidecar that renders
visibly differently in Lightroom, silently — and they join the mask losses in
the one save sentence. The other direction of the same honesty is
`xmp::global_render_gaps` (R25), the symmetric list for `CarriedOnly`: what
LIGHTROOM will render and this canvas will not. It is the precondition for the
whole carried tier existing — 24 sliders that move a number and no pixel would
otherwise be exactly the bug the inclusion laws above forbid. It compares
against `EditRecipe::default()` rather than zero, because de-fringe's Adobe
defaults are non-zero (`0/30/70/0/40/60`, this repo's first non-zero-default
`crs:` fields), and it excludes `PassThrough` on purpose: a block with no
knowable neutral cannot be judged 「active」, so keys Lightroom stamps on every
file would cry wolf forever.

Inbound, `xmp::unmodelled_global_crs` names the `crs:` properties an imported
sidecar carries on its own `rdf:Description` that this engine does not model at
all. The merge PRESERVES every one of them; what was missing was the sentence
saying they are there, so a canvas that did not match Lightroom's render had no
explanation on screen. Its universe is the COMPLEMENT of `owned_attr_keys`, so
it needs no catalogue of Adobe property names to keep up to date — and R25 is
the prediction coming true: the day the batches taught the engine `crs:Texture`,
the Grain block and the Transform/Calibration blocks, those keys joined the
owned set and left this list by themselves, with no edit to the list at all.
The cost was paid in TEST FIXTURES, which had used exactly those keys as their
「unmodelled」 samples and had to be re-based four times across the round (they
now use `PointColor`, `Look` and `CameraProfileDigest`); what remains named is
what remains unmodelled.

### 4.6 Style / eval harness (M4)

The user's **finished edits** are ground truth. If they're Lightroom XMP/develop
settings, diff the AI recipe against them; if they're exported JPEGs, compare the
AI render perceptually. Lets us measure "does the AI match *how the user*
develops a shot?" and tune the advisor prompt accordingly.

**One row changed meaning in v0.34.0** (`eval::hue_carries_colour`). The four
`color_grade.*_hue` rows now count a photo only when BOTH sides put saturation
on that wheel, above the ruler's own movement deadband. A toning wheel's hue is
an angle, and the wheel paints nothing at zero saturation, so a hue delta
between two colourless wheels was arithmetic on noise: the R27 147-photo
baseline read `shadow_hue mean|Δ| = 141`, an artifact of two neutral wheels
parked at opposite ends of a circle. The real finding it was burying is one row
down and unchanged — `shadow_sat` bias **+9.02**, the AI tinting shadows the
photographer leaves neutral. Consequences, stated: those four rows' `n` /
`mean|Δ|` / `AI-omit` are **not comparable** with any pre-v0.34.0 run (the eval
report prints that line itself), and a photo where only one side toned no
longer reaches the hue row at all — its omission is recorded on the `*_sat` row,
which is the control that carries the decision. Every other row is untouched.

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
the delivery root (see below) — **non-XMP** (pixel edits don't serialise to ACR)
— and the develop
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

### 4.10 The delivery root (v0.30.0)

Two folders, two jobs, and only one of them used to be a setting. The **develop
store** (§4.5) holds the state — recipes, XMP, versions, mask rasters — under a
per-user root that `AUTOSHOP_DATA_DIR` can move. The **delivery root** holds the
finished files, and before R24-5 it was not a setting at all: `./out`, spelled
literally in five places (the CLI deliverable name, the batch renderer's dedup
spelling, the pixel masters, the extracted style prompt, the web download
route), relative to whatever directory the app happened to be launched from —
which is precisely why "where did my export go" had no good answer.

`config::delivery_root()` is now the one reader, and `pipeline::default_out` the
one funnel every deliverable name is claimed through. It resolves the settings
file's `out_dir` over `AUTOSHOP_OUT_DIR` over the default `out`, memoised
(`unique_out` probes it up to 999 times per claim) and dropped by
`update_local_settings`, the one writer. An explicitly blank field is a real
choice — "the default" — and silences the environment variable too, the same
rule the two AI effort fields follow. The GUI exposes it in Settings beside the
develop store, echoing the RESOLVED absolute path, and its export destination's
「Delivery folder」 arm resolves through it, so this window, the CLI, the web
surface and a batch render name one place.

It is `Trust::Destination` in `config::SETTINGS`, unlike the read-only
`AUTOSHOP_LEGACY_OUT` beside it: a planted value does not merely choose a
folder, it decides where a stranger's developed photos are filed and (via
`guard_readonly`'s own-output allowance) which directory stops counting as the
read-only photo library. Neither a `.env` nor an ambient working-directory
`autoshop.local.json` may supply it.

Three things deliberately did NOT move with it, because they only ever shared
the folder NAME: the develop store (its own setting since v0.13), `out/imported`
where the web surface parks an uploaded SOURCE photo (library input, not a
deliverable), and `store::legacy_out_roots` / `AUTOSHOP_LEGACY_OUT` (a read-only
archaeology path pinned to where pre-v0.13 builds actually wrote, which a new
setting cannot retroactively change). `guard_readonly` keeps the literal `./out`
as an output area alongside the configured root, so repointing the root does not
make a `match` on an older export suddenly refused.

## 5. Why Rust — and the whole stack, named

Cross-platform, no GC pauses on large-image pipelines, first-class image crates,
single-binary distribution, trivial `std::process` shell-out to `claude`.
Toolchain in use: rustc/cargo **1.94.1** (verified locally), **edition 2024**.

The rest of this section is the complete list of what this project actually
depends on, with each entry's REASON — because "first-class image crates" is a
slogan, and a public repository that is being copyright registered should be
able to answer "what is in it" without anyone reading `Cargo.lock`. Every crate
below is a DIRECT dependency in [`Cargo.toml`](../Cargo.toml), whose own
comments are the source of these one-liners.

**Runtime (both binaries).**

| crate | why |
|---|---|
| `anyhow` 1.0.103 | the application error type everything fallible returns |
| `base64` 0.22 | the preview image goes to the vision API as a base64 `input_image` data URL, and images come back the same way |
| `bytemuck` 1 | zero-copy `Vec<[f32;3]>` ↔ `Vec<f32>` casts in the orientation stage — a 61 MP portrait RAW otherwise pays three ~732 MB full-frame copies (`render::orient_f32`) |
| `clap` 4.6.1 (`derive`) | the CLI surface: subcommands, `--jobs`, `--strength`, the rest |
| `dotenvy` 0.15 | reads `.env` — under the trust table of §3, which is why a `.env` may carry a `Secret` and not a `Destination` |
| `getrandom` 0.3.4 | CSPRNG bytes for the `serve` session token gating image URLs; anything seeded from the clock is guessable, which is the whole attack. Already transitive, so no new dependency |
| `image` 0.25 | baked-source decode + every export encode. `default-features = false` and the codec set is opt-in one at a time — `jpeg`, `png`, `tiff`, `webp`, `bmp`, `gif`. avif/heic stay OUT because they mean a C toolchain (dav1d) this tree does not have; R27 added the last three only after checking each one's dependency closure (all pure Rust, no `build.rs`, no bundled C) |
| `qcms` 0.3 | ICC → sRGB for baked imports; LR's "Edit in…" exports ProPhoto 16-bit TIFFs, and treating those pixels as sRGB fed every tone decision wrong numbers. Pure Rust |
| `rawler` 0.7.2 | RAW decode — 24 formats, 725 camera models, embedded preview, full EXIF, as-shot WB (§4.1) |
| `rayon` 1 | row-parallel per-pixel stages; it was already in the tree (rawler's demosaic uses it), so our serial tail joins the same pool. Every conversion is per-pixel independent, so outputs stay bit-identical |
| `serde` 1.0.228 (`derive`) + `serde_json` 1.0.150 | `EditRecipe`, `recipe.json`, `variants.json`, the style index, and every AI request/response body |
| `thiserror` 2.0.18 | the typed error enums whose arms callers branch on — `advisor::AdvisorError` is the one that matters (a CLI envelope failure and a bad verdict are different recoveries) |
| `tiny_http` 0.12.0 | the local web server behind `serve` (§4.9) |
| `ureq` 2 (`json`) | the blocking HTTP client for every AI endpoint, including the SSE streaming arm |

**Native GUI**, behind the `gui` feature so a plain `cargo build` / `cargo test`
never pulls in winit and the GL stack: `eframe` + `egui` **0.29** (the desktop
app, with `eframe/persistence` remembering window geometry and our own prefs),
`rfd` 0.15 (native file/folder dialogs), and `ab_glyph` 0.2 — the same version
egui pulls — used to pre-validate a CJK font before handing it to egui, which
panics on an unparseable one, so font loading degrades safely.

**Platform.** `windows-sys` 0.59 with five feature gates:
`Win32_Foundation` + `Win32_System_JobObjects` for the sidecar kill-groups (a
timeout must reap the sidecar's WHOLE process tree), `Win32_Security` +
`Win32_System_Threading` for `CreateJobObjectW`'s security-attributes parameter
and the extended limit struct's `IO_COUNTERS`, and
`Win32_System_SystemInformation` for `GlobalMemoryStatusEx` — the free-memory
reading R27's `--jobs` cap divides by its measured per-photo peak. On unix,
`libc` 0.2 does the two same jobs (`killpg` for the kill-group, `sysconf`
`_SC_AVPHYS_PAGES` for free memory). Both crates were already in `Cargo.lock`
transitively, so promoting them added no download and no new supply-chain
surface. Build-time: `winresource` 0.1 embeds the Windows app icon
([`build.rs`](../build.rs)), best-effort — no resource compiler still builds.

**Dev-only.** `tiff` 0.11 — the same version `image` already locks — and, on
Windows, `windows-sys` 0.59 a second time with `Win32_System_ProcessStatus`
(`GetProcessMemoryInfo`): the `#[ignore]`d `jobs::probe_per_photo_peak_commit`
harness that re-derives `PER_PHOTO_PEAK_COMMIT_MB`; resolver 3 keeps
dev-dependency features out of the shipped binaries. `image`'s
own TIFF *encoder* cannot round-trip an `IccProfile` tag (probed: the written
tag reads back `None`), so the ICC regression tests write their fixture through
the `tiff` crate directly, the way Lightroom-style writers produce real profiled
TIFFs.

**The Python sidecar stack** is deliberately NOT in `Cargo.toml`: three scripts
under `python/`, shelled out to with `-E`, each doing one job and each failing
loudly rather than degrading silently (`lib.rs::sidecar_wrote`). They need
Python 3 + PyTorch (CUDA where the box has it), plus `transformers` on the sky
and object paths and `rembg` on the subject path. The five models they load,
their licences and their pinned byte counts are the 「ML sidecar family」 table
at the top of this document — not repeated here, so there is one place to
correct. Weights are never in this repository: they are fetched on first use
and pinned in the two tiers that table describes — sha256 plus an exact byte
count where the sidecar fetches every file itself, an HF revision only on the
two older segmenter paths, which say so in their own comments.

**The AI services** are two HTTP contracts and one subprocess: an
OpenAI-compatible **Responses** API for the vision advisor and the visual judge
(strict `json_schema`, `store: false`), the **Images** API (`/v1/images/edits`,
`gpt-image-*`) for the experimental generative edits, and the **`claude` CLI**
for the analyst/verifier role — reusing Claude Code OAuth, so that role needs no
API key at all (§3, §4.3).

**The web front-end** is one file: `src/web/index.html`, `include_str!`'d into
the binary and served by `tiny_http`. Vanilla JS, one inline `<script>`, zero
build step, zero CDN — which is also what makes §4.9's origin rules the whole
of the client's trust story.

**The XMP layer is hand-rolled** — [`src/xmp.rs`](../src/xmp.rs) reads and
writes the document itself, and this project takes no XML crate as a
dependency. That is a choice, not an omission: a Lightroom sidecar must be
*merged into*, preserving byte-for-byte everything this engine does not model
(`xmp::insert_crs_description`, the namespace SCOPE STACK of v0.23.3, the
pass-through key set), and a serialise-from-a-DOM round trip cannot promise
that.

## 6. Open questions

| # | Question | Status |
|---|----------|--------|
| 1 | **Image library path** (originals + finished edits) | resolved: passed per invocation (`batch <dir>`, `serve --dir`, `style-index <dir>`, the GUI folder picker) — no configured library root; develop state is keyed by each photo's absolute path in the per-user store. One exception since R23-2: the STYLE library's source folder is remembered in the GUI prefs (`style_src_dir`) so a rebuild need not re-find it — a convenience, not a library root; the index records the folder it was built from as well |
| 2 | Camera / RAW format | ~~resolved: Sony `.ARW`~~ **← R27 (2026-08-19) widened**: 24 RAW extensions (`decode::RAW_EXTS`) + 8 baked (`pipeline::BAKED_EXTS`), one predicate app-wide; 9 cameras (one per format) verified end to end on CC0 samples, and the zoo is a release gate (`AUTOSHOP_RAW_ZOO`, 9/9 at v0.33.0); Adobe DNG Converter is the documented on-ramp for the rest. Refusals are named per cause (unknown make / unknown model / no decoder); monochrome and 4-colour sensors are refused before the develop; X-Trans develops approximately and says so |
| 3 | Output target | resolved: XMP sidecar **+** rendered, XMP-first |
| 4 | AI roles | resolved: GPT=image, Claude=non-image+verify, unified framework |
| 5 | Exact meaning of Claude's "收货验证" (data-level vs pixel-level) | resolved: **data-level**. The verifier is never sent pixels — it judges the recipe against EXIF/histogram/clipping stats and the advisor's rationale (§3, §4.3, [`src/advisor/claude.rs`](../src/advisor/claude.rs)) |
| 6 | How to feed the preview to the GPT vision API; `crs:` key set for ARW | resolved in shipped code: the preview goes as a base64 `input_image` data URL on the Responses API with a strict `json_schema` ([`src/advisor/openai.rs`](../src/advisor/openai.rs)); the ARW `crs:` key set is the one the writer emits and round-trips ([`src/xmp.rs`](../src/xmp.rs)) |
