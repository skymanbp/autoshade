# AutoShade — Architecture

> Status: **implemented** (v1.2.3 — the hue-fan veto and the cast projection,
> the direction-led Style voice, the range boundary budget measured and kept,
> four rationale notes reaching the Chinese UI; v1.2.2 was the contextual
> boundary budget, the embedded-preview-is-not-the-frame class, the style
> index's one band table, `--xmp-dir` and incremental index builds). The
> reverse-fit's
> in-range estimator is now a PAIRED ROBUST REGRESSION (2026-08-26): on
> same-frame pairs the tone map is estimated from corresponding pixels
> (per-bin Tukey-IRLS means, median start, robust weight × evidence weight),
> candidates are model-selected through the engine's own spline at the map
> points, knots and residual-curve levels outside the measured luma span
> carry no weight, and every hue-damage guard consults a per-pixel voucher
> (robust weight + hue coherence + movement toward the pixel's own paired
> target) so undoing a cast is no longer vetoed by the bands the cast
> invented, while incoherent content divergence keeps every veto. What the
> estimator rejects, and what vouched convergence carried through one-sided
> bands, are both typed disclosure notes. Since v0.35.0, R30 adds
> guarded modern `MaskBrushTable` import, gesture-aware SAM point prompts with
> a scoped cache re-key, and stronger eval/error disclosures. D1 changes angled
> LINEAR masks to the pixel/aspect metric (`ecb6505`). D2 establishes the
> `(i+1)/16` camera-knot law, the Lightroom radial centre and exact-once
> transport (`706ac84`), then splits LINEAR onto the measured H2
> handle-transport topology (`ad6de62`); the two new persisted frame facts are
> deliberate forward schema breaks. This is atop R29's measured radial
> falloff, negative Texture, `.lcp` reader, pixel-centre mask sampling and
> BiRefNet subject path, plus R28/R27's input and safety work. The accepted
> input set remains **24 RAW extensions + 8 baked formats**
> (`decode::RAW_EXTS` / `pipeline::BAKED_EXTS`, one predicate app-wide), with
> nine cameras — one per format — run end to end from CC0 sample files.
> `batch` and `eval` gained **memory-budgeted `--jobs N` parallelism**
> ([`src/jobs.rs`](../src/jobs.rs)): one 61 MP photo's pipeline pass peaks at
> ~1.77 GB of commit charge, so the worker count is capped by free memory and
> DISCLOSES when the cap overrules the flag — the 147-photo eval went from
> ~2.3 h serial to a measured 38 min at `--jobs 3`. The GUI applies the same
> discipline (`src/bin/gui/budget.rs`, 2026-08-25): every worker that pays a
> full-frame peak takes a byte reservation before the expensive call and QUEUES
> — never refuses, never downscales — while the machine's current free memory
> cannot carry it beside a 2 GB reserve, and on an already-tight machine the
> global rayon pool starts clamped to 8 workers. Lightroom **brush and AI
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
> pipeline ships across three front ends — a native desktop GUI (`autoshade-gui`,
> egui/eframe, which links this library in-process), the local web UI (`serve`),
> and the CLI — plus AI denoise (SCUNet sidecar), the PNG/TIFF
> baked-source mode, style retrieval, XMP sidecars (global + local masks),
> experimental generative edits, an optional pixel-**heal** retouch mode (§4.7)
> the deterministic look **reverse-fit** (§4.8) and the local server's refusal
> model (§4.9).
> The battery is one command, `scripts/release_battery.sh`: a **default** lane
> (`cargo test`, the corpus variable unset), a **gui** lane (the
> `autoshade-gui` bin under the `gui` feature) and a **calib** lane (the
> library a second time with `AUTOSHADE_FIT_CALIBRATION_DIR` at the p36-p39
> corpus and `--nocapture`, which is the only way a skipped test's own line
> reaches the transcript at all), each in its own target and data directory
> and all three in parallel. It refuses to start at all when the corpus is
> absent, because a battery whose corpus-gated tests skip is green for the
> wrong reason; it writes a `=== name ===` transcript that
> `scripts/check_docs.py --gates` reads the counts and the lane set back out
> of, and it prints the by-name test-set difference against a saved baseline.
> 1332 library + 23 CLI + 160 GUI + 2+2 contract tests are enumerated in the GUI
> build; the library result is 1320 pass + 12 `#[ignore]`d forensic probes
> (counts refreshed 2026-09-02 for v1.2.3: +24 / −0 by name against `8e631f7` —
> the five hue-fan gate tests and the nine projection tests in `fit::tests`
> (one of which replaces `cast_curves_must_not_fan_a_coherent_sky_across_luminance`),
> the range-rim budget window test, and the seven direction-led Style tests
> (`StyleVoice`, the judge's brief, the blend-site guard, the web `adherence`
> field); before that, v1.2.2 was +29 / −1 against `d628c80` — 22 from the
> style-index batch, the two contextual-budget boundary arms, the size-plan
> and base-look frame tests, the two band-table tests, and the same-content
> diagnosis test re-pointed to the Cornwall panel; the GUI
> battery is +1, the sidecar-folder preference; before that, v1.2.1's release
> run of disclosure fixes was net +5 —
> `a_truncated_rationale_says_how_much_it_lost` and
> `look_weight_is_a_real_ratio_against_the_direction_terms` (`13cebf9`), the
> neutral-solution exit's own note and the post-stamp domain disclosure
> (`693917b`), and `a_build_that_does_not_adopt_leaves_the_old_folder_strictly_alone`,
> the store adoption's refusal branch, which the macOS opt-out had until then
> left executed on no platform at all (`410993e`) — so library 1263→1268;
> before it, step 9 is net +10 — it added 11 named tests
> (2 `fit` for the per-pixel white-balance statistic and for a readable
> correspondence field choosing the PAIRING as well as the population, 7
> `fit_zoned` for the introduced-rim boundary arms and the k=0 render
> invariant, 2 for the inert-attachment refusal on each of the two boundary
> gates) and RENAMED one, `a_rim_that_cannot_be_shrunk_is_dropped_with_its_own_note`
> becoming `a_zero_dialled_mask_that_is_not_a_render_no_op_is_dropped_with_its_own_note`
> because the differential rim makes the old premise (a scene rim the
> correction did not make) a case the gate must now KEEP; before it, R30 R2
> added 8 named tests for the
> shared-content reference population and adjudication added a ninth, closing
> the two-sided retention floor and the target mask's provenance, so library
> 1256→1265; before it, the clearing dedup batch was net +1 — it added
> the adherence-tier prompt pin and moved the SHA-256 known-answer test into
> the hash's own module, retiring `eval`'s duplicate vector test and narrowing
> what stays there to the avalanche property its stale-sidecar guard rests on,
> so library 1255→1256; before it, the C2 storage-name migration added 3 named
> tests — 1 `serve` for the export-registry adoption, 2 GUI for the prefs
> adoption and the per-platform key decision — so library 1254→1255 and
> GUI 157→159; before it, R30 batch 3 added 9 named tests against
> `2a415a5` — 5 `advisor` for the colour guardrail pair, the templated
> neutral hatch, the numeric curve/mask freedoms, the three-band snapshot
> and the judge's palette item, 1 `advisor::mod` for the verifier checklist,
> 3 `mask_habit` for the hue habit dimension — so library 1245→1254;
> before it, R30 batch 2 added 18 named tests against
> `a31eb2f` — 15 `style` for the vocabulary-complete distillation (one of
> them the env-gated `#[ignore]` calibration harness) and 3
> `advisor::catalogue` for the mutable accessors — and renamed one whose
> promise the batch narrowed (`retrieval_and_style_targets_do_not_read_
> mask_habits` → `retrieval_does_not_read_mask_habits`), so library
> 1228→1245; before it, the macOS port M1-M3 added 18 named tests by
> name against `df62554` — 12 library (8 `config` for the interpreter, the
> bundle boundary and the weight cache, 3 `store` for the per-platform store
> name and the off-Windows device-path refusal, 1 `lib` for the source-cut
> rule those platform assertions depend on) and 6 GUI (5 `quit` for the ⌘Q
> state machine, 1 `i18n` for the platform modifier label) — so library
> 1216→1228 and GUI 151→157 at the merge, with CLI and contract unchanged;
> before it, R30 batch 1 added 8 named tests against `52fb38c` — 5
> `fit_zoned` for the strictly-better arm, 3 `fit` for the R2-lite
> reference-population disclosure — and the supervising merge added its
> Atmosphere do-no-harm pin, so library 1207→1216 with CLI, GUI and
> contract unchanged; before it, the AutoShade rename added 14 named tests by
> name against `e33206b` — 6 for the environment alias door, 5 for the
> pre-rename develop-store adoption, 3 for the pre-rename on-disk XMP tokens —
> and renamed one (`xmp::tests::legacy_autoshop_sidecar_…` →
> `legacy_autoshade_sidecar_…`), so library 1193→1207 with CLI, GUI and
> contract unchanged; before it, the tile-boundary root fix added its two named
> gate tests — library 1191→1193; before it, the style-retrieval expansion
> `style-s2` (+55 by name against the `32b0fe4` merge transcript: 46 library —
> 26 `style`, 9 `describe`, 4 `pipeline`, 3 `embed`, 3 `advisor`, 1 `recipe` —
> 4 CLI and 5 GUI; the gui trip now runs the GUI bin only, the `gui` feature
> adding dependencies alone): library 1091→1137→1191→1193, CLI 16→20→22, GUI 146→151;
> before it the multi-region batch `a2173c9`
> (+24 by name against `6323f4c`: the 11 `fit_zoned::semantic` tests, the 7
> `fit_zoned` routing / arbitration / raster-release tests and the 6 `segment`
> multi-class manifest tests) onto the linear-falloff merge: library 1067→1091;
> before it the linear-falloff line `817fa13`
> (+10 by name against `662b688`: the C¹ ramp harness `56dd690` and the `Eased`
> flip — `linear_mask_renders_the_eased_ramp`, `shipped_linear_ramp_is_eased_end_to_end`,
> the radial/bitmap split of the clamped-baseline test) and the step-17 cleanup
> batch `9097319` (+2 by name = `rationale::tests::sidecar_failure_disclosure_has_no_traceback_or_home_path`
> and `fixture_dir_tests::test_fixture_dirs_are_process_unique`) merged onto the F1
> strength-axis batch `302efb1` took the library 1034→1067; F1 alone went
> 1034→1055, set diff +22/−0 default and +23/−0 GUI by name against `662b688`
> = the strength-budget, WB-manifold, rescoring-disclosure and Style-wording
> tests in `src/fit.rs`, `src/style.rs`, `src/main.rs` and the GUI panel pin; the free-mask batch
> `662b688` before it went 1017→1034, +17/−0 against `d21304a` = the 17 tests in
> [`src/fit_zoned/freemask/tests.rs`](../src/fit_zoned/freemask/tests.rs); the
> local-field batch `d21304a` before it went 991→1017, +26/−0 against `10e02bb`,
> no status change — added,
> in [`src/fit_field.rs`](../src/fit_field.rs),
> `field_splat_is_a_partition_of_unity`, `field_adjoint_matches_forward`,
> `field_infinite_tikhonov_reproduces_the_global_render`,
> `field_identity_pair_is_all_zero`, `field_solve_is_deterministic`,
> `field_refuses_without_evidence`,
> `field_recovers_a_planted_two_band_exposure`,
> `field_band_dispersion_flags_spatially_structured_bins`,
> `calibration_field_ceiling_matches_the_numpy_solver`,
> `the_local_field_never_reaches_the_engine_or_the_recipe_schema`,
> `calibration_local_support_is_not_constant` and the two new
> `#[ignore]`d probes `export_calibration_field_inputs_for_numpy` and
> `compare_calibration_field_with_numpy`; in
> [`src/fit_zoned/field.rs`](../src/fit_zoned/field.rs),
> `field_band_proposal_matches_a_two_band_remap`,
> `field_band_proposal_skips_a_spatially_structured_bin`,
> `field_shape_reads_a_bright_quadrant_as_tile_shaped`,
> `field_shape_ignores_unmeasured_pixels`,
> `field_shape_reads_a_diagonal_ramp_as_linear`,
> `field_stop_and_realized_helpers_are_well_conditioned`; in
> `src/fit_zoned/range.rs`, `field_proposals_enter_before_range_sort_and_cap`,
> `field_proposal_union_merges_same_sign_and_refuses_opposite_overlap` and
> `field_proposal_spans_are_mapped_through_the_pixels_that_occupy_them`; in
> `src/fit_zoned/spatial.rs`, `tile_attachment_cap_is_parameterized`; and in
> `src/fit_zoned.rs`, `field_disabled_layer_is_byte_identical`,
> `field_stop_rule_skips_the_tile_producer_and_names_it` and
> `calibration_local_field_discloses_ceiling_and_realized_share`; nothing
> removed). THREE suites are ADDITIONAL and env-gated, so a bare `cargo test`
> does not include them. The F1 release reproof adds +22 default test names
> and +23 GUI names with no removals or status changes against the B3
> transcript `target/b3-main/gates-final.txt`.
> `AUTOSHADE_LR_PROBE_FIXTURES` (16 real Lightroom radial sidecars, byte
> round-trip), `AUTOSHADE_MB_FIXTURES` (the 7-file M-B forensic set — 42 of its
> 42 corrections imported, 0 refused) and, since R27, `AUTOSHADE_RAW_ZOO` (the
> CC0 nine-camera zoo, one RAW per format —
> `every_make_in_the_raw_zoo_decodes_and_agrees_with_itself` in
> [`src/decode.rs`](../src/decode.rs), 9/9 at the last release). Every release
> runs all three and records their own counts, rather than carrying the previous
> release's forward — see ROADMAP「发版链 + 环境门套件」.
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
> endpoint, all sandboxed via `AUTOSHADE_DATA_DIR`); v0.21.0 extended it to the
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
>   that distinction is the point. `Trust` classifies AutoShade's own settings —
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
>   same reason `AUTOSHADE_WEIGHTS_DIR` is `Destination`) and the proxy
>   variables (a proxy decides where bytes go). The reach this costs is small
>   and recoverable: a child INHERITS the parent's environment — nothing calls
>   `env_clear` — so a user's own `HF_HOME` or `HTTPS_PROXY` still arrives
>   untouched. Only a `.env`'s attempt to add or override one is refused, and
>   it says so.
>
>   It replaced three hand-kept lists that had provably drifted: the guard's
>   own test carried a copied 14-name array while the constant had grown to
>   17, and `Config::load` read that array BY INDEX (`pre(11)` meant
>   `AUTOSHADE_OPENAI_MODEL`), so adding or removing one name silently
>   repointed unrelated config fields at the wrong variable. The `Destination`
>   half is the one that matters most: `AUTOSHADE_CLAUDE_BIN` and
>   `AUTOSHADE_PYTHON` reach `Command::new` verbatim and the script variables
>   become that command's argv, so guarding only the base URLs left the
>   strictly worse outcome open on the same file. `ANTHROPIC_API_KEY` /
>   `_AUTH_TOKEN` / `_BASE_URL` are `Destination` too — for the `claude` child
>   the credential IS the routing decision.
>
>   Resolution is per FIELD, so a planted `autoshade.local.json` carrying only
>   `image_base_url` used to redirect the endpoint while the real key still
>   came from the environment — the filesystem twin of the cross-origin hole
>   §4.9 describes, and it needed nothing but running AutoShade inside an
>   extracted archive. Four routes to that outcome are closed: the read path
>   (v0.18.0), the settings-SAVE path — which read-merge-wrote ambient values
>   into the trusted central file, where nothing strips them again — `.env`,
>   which `dotenvy` searches for from the working directory upward, and the
>   STORE ROOT itself (v0.23.2): the per-user directory used to be
>   `%LOCALAPPDATA%` on every platform, a variable Unix does not set, so every
>   Linux/macOS build fell through to `/tmp/autoshade` and granted the settings
>   file found there full central authority. Each platform now names its own
>   per-account directory (`$XDG_DATA_HOME`, `$HOME/.local/share`), and a
>   shared-temp fallback is LABELLED (`store::RootTrust::SharedFallback`) so
>   the loader downgrades it to ambient rather than trusting it.
>
>   A `.env` keeps `Secret` on purpose — it is where this project's own key
>   lives, a documented contract — which is also why a `.env` picking
>   `AUTOSHADE_ANALYSIS_PROVIDER` is not an escalation: supplying
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
>   AutoShade's own earlier projection made a REAL loss produce no note at all.
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
> e.g. one denoised in Lightroom — auto-detected by file type). All four sidecar
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
> ### The ML sidecar family (R27 Batch-5; S2 added the fifth)
>
> There are now **five** Python sidecars, and they share one discipline rather
> than five copies of it. `python/denoise.py` owns the download-and-refuse
> implementation — `_download` with an in-stream byte cap, `_sha256`,
> `_reclaim_stale_parts`, `_fetch_verified` — and the other four reach it
> instead of reimplementing it, which is why their progress lines announce
> themselves as `[denoise]`.
>
> That discipline lives in **three** shared modules, not in each script.
> [`python/_device.py`](../python/_device.py) is the one `cuda` -> `mps` ->
> `cpu` ladder (M2). [`python/_sidecar.py`](../python/_sidecar.py) is what the
> three single-artifact sidecars — `describe.py`, `embed.py`, `correspond.py` —
> had each written out for themselves: the progress line, the refusing exit
> (always 2, so a refusal is distinguishable from a crash), the
> pinned-revision directory, the fetch across the pin table, and the
> `tmp`+`fsync`+`replace` publish. It imports `_fetch_verified` from
> `denoise.py` on their behalf, so the paragraph above still says where that
> implementation is. What stays per-script is which MODEL and which name to
> say, which is all each one now binds — every call site kept its spelling.
> Two copies deliberately remain outside: `denoise.log`, because `_sidecar`
> imports denoise and the reverse would be a cycle, and `segment.die`, because
> segment.py loads no heavy module at import time and `_sidecar` pulls in
> numpy — a two-line saving is not worth putting that on every
> `segment.py --help`.
>
> | sidecar | bridge | model(s) | licence | size |
> |---|---|---|---|---|
> | `denoise.py` | `denoise.rs` | SCUNet ×5 | Apache-2.0 (KAIR) | ~72 MB each |
> | `segment.py --target subject` | `segment.rs` | **BiRefNet** (general checkpoint), sha256-pinned | MIT | 444,473,596 B |
> | `segment.py --target subject` (fallback) | `segment.rs` | U²-Net via a NAMED rembg session | Apache-2.0 | small |
> | `segment.py --target sky` | `segment.rs` | **OneFormer ADE20K Swin-L**, sha256-pinned | MIT (weights) | 881,196,376 B |
> | `segment.py --target sky` (class table) | `segment.rs` | ADE20K label table the processor requires — **ours, rebuilt from the model's own MIT metadata**, ships in `python/`, not downloaded | MIT (sources) | 7,085 B |
> | `segment.py --target object` | `segment.rs` | **SAM 2.1 Hiera-Large**, point-prompted | Apache-2.0 | 897,897,416 B |
> | `embed.py` | `embed.rs` | **SigLIP 2 base/16 @384**, 768-dim | Apache-2.0 | 1,501,968,264 B |
> | `correspond.py` | `correspond.rs` | **Stable Diffusion 2.1** as a DIFT featurizer (unet+vae+text encoder, fp16), sha256-pinned | CreativeML Open RAIL++-M | 2,580,061,174 B |
> | `describe.py` | `describe.rs` | **Qwen3-VL-2B-Instruct**, one grade sentence per photo, sha256-pinned | Apache-2.0 | 4,255,140,312 B |
>
> **Licence is a selection criterion, not a footnote.** This is a public
> repository whose product is being copyright registered, and a licence that
> restricts *use* is not cured by not redistributing the weights. SegFormer was
> removed in Batch-4 for exactly that (「for research or evaluation purposes
> only」); CLIP and OpenCLIP were passed over in Batch-5 because their model
> cards say deployment is out of scope and the OpenAI HF mirror carries no
> licence tag at all. In both cases the licence-clean option was also the
> stronger model. `describe.py`'s Qwen3-VL-2B (S2) was chosen on the same
> criterion twice over: it is Apache-2.0 and ungated, and — unlike Florence-2,
> the obvious MIT alternative — its repository ships **no `auto_map`**, so it
> loads through NAMED transformers classes instead of `trust_remote_code`,
> which downloads and executes upstream Python through HF's cache where this
> family's digest gate cannot see it. BLIP-family captioners were passed over
> on fitness rather than licence: they are subject captioners, and a subject is
> the one thing this sidecar must never emit. A paid vision API was rejected by
> the user's own ruling — a library rebuild that is otherwise free and offline
> must not acquire a per-photograph bill, and the photographs must not leave
> the machine to produce a field that lives in a local index.
>
> stronger model. `correspond.py`'s SD 2.1 (step 7a) is the checkpoint the
> DIFT paper measured; its RAIL++-M licence **allows commercial use** — the
> restrictions it carries are conduct-based (unlawful-use clauses that travel
> with the weights), not field-of-use, which is the line SegFormer fell on.
> The official `stabilityai` repo being **delisted upstream** (verified
> 2026-08-26: anonymous 401, authenticated 404) is why the pin names a
> community mirror — adopted only after its fp32 tower digests proved
> byte-identical to an independent uploader's, and the sha256 gate below is
> the only door at run time either way. BiRefNet (R29 B4) is MIT, unmodified and ungated on both the
> weight repo and the code repository it points at — and the checkpoint chosen
> is the GENERAL one, not the `_HR-matting` variant the R27 design document
> named: measured on the photographer's own library, HR-matting returns an
> empty alpha on 4 of 9 real frames, so adopting it as designed would have
> deleted masks rather than approximated them.
>
> **Pinning is now ONE tier, and closing the last gap found a second one
> (R29 C3/C4).** `denoise.py`, `embed.py`, `correspond.py`, `describe.py` and every `segment.py` backend fetch
> every file themselves, gate it on sha256 + an exact byte count, and load from
> a local directory with `local_files_only=True` — the digest is the only door.
> For BiRefNet that gate covers a file that is **executed**: `birefnet.py` is
> the model's own source, loaded through `importlib`, so the digest is what
> stands between upstream and `exec_module`. `trust_remote_code` is never used
> anywhere: it downloads and executes upstream Python through HF's cache, which
> our gate never sees.
>
> Sky was the last holdout — an HF *revision* pin alone, which fixes WHICH tree
> is fetched but not the BYTES — and the reason it was not a four-line copy of
> the BiRefNet gate turned out to be worse than the tokenizer tree it was
> registered as. `OneFormerImageProcessor.__init__` (and the Fast variant alike)
> ends in `load_metadata(repo_path, class_info_file)`, which falls through to
> the hub downloader against **`shi-labs/oneformer_demo`, a different repository,
> a DATASET repo, at its moving `main`** — on every single sky mask.
> `SKY_REVISION` never reached it, `local_files_only` does not stop it (it is a
> separate call with its own kwargs), and the `metadata` key sitting in the
> pinned `preprocessor_config.json` is filtered out and recomputed from the
> download. Proved by running the load under `HF_HUB_OFFLINE=1`, which died on
> exactly that URL. All seven weight/tokenizer files are now fetched and
> digest-gated here, and `repo_path` points at the verified directory so the
> metadata load takes its local branch; the same probe now completes offline.
>
> **And then the class table stopped being fetched at all (R29 收口, ruling
> 11).** `shi-labs/oneformer_demo` declares NO licence (`cardData: null`, tags
> `["region:us"]`), so of every asset this tree pulled it was the only one that
> had never been through the criterion above — pinning its revision fixed which
> bytes arrived, not the terms they arrived under. The user's ruling was to
> replace it with a table of our own, so `python/ade20k_class_table.json` is
> **built from licence-clean facts and shipped in the repo**: class names and
> ids from the MIT model repo's own `config.json` `id2label`, the thing/stuff
> split from that same repo's `preprocessor_config.json` `metadata.thing_ids`
> (the field transformers filters out of the kwargs and recomputes — its
> *content* was never the problem), cross-checked row by row against SHI-Labs/
> OneFormer's MIT `ADE20K_150_CATEGORIES`. All three sources agree on all 150
> rows. It is not a byte copy of the file it replaces — different key order,
> different formatting, 7,085 B against 7,084 — but it is **equivalent where it
> counts, proved at the pixel level**: one frame, two full sky runs, old table
> against ours, byte-identical mask PNGs. So `AI_BACKEND_GENERATION` does **not**
> move and no cached alpha needs re-deriving. The digest gate stays — it now
> pins a file in this repository rather than a download, because a half-written
> checkout is exactly as bad as a moving branch, and `python/*.json` is pinned
> to `eol=lf` in `.gitattributes` so a Windows checkout cannot fail that digest
> on a tree git considers identical.
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
> Rust side checks moves; measured against an fp32 twin of the same
> 169-exemplar library, `1 - cos` is at most 2.235e-5, the top-5 is identical
> for 168 of 169 leave-one-out queries and no retrieved SET moves at all — the
> one place the drift is visible is the tag argmax, which flips a phrase for 6
> of the 169), and calls are SINGLE-FLIGHTED behind a process-wide
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
> `crs:What="Mask/Image"` carries no raster and no geometry — 105 real
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

- **Reproducibility** — same recipe + same RAW + the same build ⇒
  byte-identical output, on every run. Across builds the dial layer is stable
  but the pixels are not promised: no toolchain is pinned (see TECH_STACK
  §Parameters), so a different compiler may round differently.
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
refine) bakes a freshly claimed file and repoints the recipe. Deletable rasters
are typed: the zoned reverse-fit's cleanup sites take `store::OwnedRaster`,
constructible only from a fresh claim (or an explicit test scratch that refuses
the calibration corpus), so a borrowed user path handed to a deleting call site
fails to compile — the contract that used to live in a doc comment and, on
2026-08-25, let a test hand the user's calibration mask to the cleanup path.

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
| 1 | Proposer prompt | `advisor::openai::{strength_clause, guardrail_pair, colour_guardrail_pair, curve_guardrail, look_coverage_clause, colour_neutral_clause, mixer_restraint_clause}` | Three banded restraint templates; the quoted ±Highlights/Shadows and ±Whites/Blacks pair opens from the measured ±50/±35 up to ±75/±55; since G2/G3 **colour carries numbers on the same axis** — an `hsl` band ±10–20 → ±20–40, the four colour-grade wheels' saturation TOTAL 20–40 → 40–80, the tone curve 3–5 → 7–9 points at a depth of 8–15 → 15–30 of 255 and a per-channel curve 4–8 → 8–15 — and the colour-neutral escape hatch stops being unconditional above the restrained band; "most photos need only a couple of HSL bands" becomes an explicit per-control decision |
| 2 | `EditRecipe::temper` | [`src/recipe.rs`](../src/recipe.rs) | The four soft-cap knees/ceilings scale by `1 + (s − 0.5)·0.7` — 0.5 is the shipped 50→70 / 30→45 exactly, and full strength asymptotes at 94.5, still inside the ±100 hard `clamp` |
| 3 | Verifier prompt | `advisor::{verify_flat_clause, verify_cooked_clause}` | The too-FLAT band tightens, the OVER-COOKED band relaxes; the target and the photographer's DIRECTION are stated ABOVE the checklist they modify |
| 4 | Visual judge rubric | `advisor::judge::intent_rubric` | The Develop rubric gains the target and the direction. FitMatch gains neither — a look MATCH has no strength |
| 5 | Style reference wording | `style::render_reference` (+ gate 1's reference clause) | Below Style 0.85 the retrieved habit is a CEILING ("not stronger", "do not exceed it"); at or above it, a FLOOR headed "TARGET style to reproduce". Since v1.2.3 a THIRD voice, `StyleVoice::Background`, is chosen instead whenever a non-empty direction is present at adherence `Direct`/`Brief` — header "STYLE BACKGROUND … The DIRECTION LEADS", and every aim clause hands the decision to the direction (§4.6). This gate is templated on the STYLE axis, not on this dial: strength above 0.70 with Style below 0.85 no longer receives the old committed-tier FLOOR wording. The measured NUMBERS never change in any voice — a dial must not restate what the photographer actually did |
| 6 | No-AI fallback | `advisor::heuristic` | The baseline's histogram-driven recovery goes through the same `temper` dial, so the fallback cannot taste different from the AI path at one setting |

Bands are coarse (≤ 0.4 restrained / ≤ 0.7 balanced / above committed) because
prose cannot be interpolated; every NUMBER on the axis is continuous. One
consequence worth knowing: 0.50 and 0.65 share the balanced band, so they differ
in the guardrail numbers and `temper`'s knees, not in the adjectives.

Two colour changes from the same round are deliberately **not** on the dial,
because neither is a question of how hard to push. The visual judge's BASE
`Develop` rubric gained a POSITIVE colour item beside its existing `colour
health` fault item ("a deliberately designed palette is a strength; judge
whether the colour decisions are coherent, never whether they are small") — the
one-sided rubric asked 30 guided revisions for less colour 17 times and for more
0 times, and a rubric where the only rewardable colour move is not making one is
broken at every strength. The data-only verifier's checklist gained a colour
COMPLETENESS item and a curve MONOTONICITY item for the same reason: its four
measured revisions all pushed in the right direction ("commit harder") through a
checklist that was 100 % tonal. The monotonicity item is a CHECK, not a clamp —
the `MAX_CURVE_POINTS = 256` clamp local to `EditRecipe::clamp` is unchanged
and `temper` gains nothing, exactly as
the fit side's `project_curve_slopes` shapes a proposal rather than bounding the
renderer.

Two things the axis must never touch, both measured defects rather than taste:
`temper`'s **white-point coupling** (`whites ← −highlights·0.3`, global and per
mask — `bd3f9d4` fixed a recipe that dragged sea foam to grey), and the prompt's
matching "recovering highlights must NOT grey out specular whites" rule, which
stays unconditional at every strength. `clamp`'s hard ranges are a safety bound
and are not on the axis either. The **Style** slider's `style_pull` (0.18 at the
shipped Style 0.3, full at Style 1.0) is off the strength axis on purpose too:
that number bounds mean-regression toward the user's
own average edit, so coupling it to strength would turn "push harder" into "look
more like my average" — the other axis, pointing the other way. Since v1.2.3 there
is exactly one case in which that pull does not run at all — a direction leading at
adherence `Direct`/`Brief` (`pipeline::style_blend_pull`, §4.6). That is not a
strength decision either; it is the same `StyleVoice` the wording above reads, so
the block and the arithmetic cannot disagree about who leads.

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

Provider/model/key selection lives in `autoshade.local.json` (written by the
Settings panel) and/or `.env` — both gitignored; the local file overrides env.
That file lives in the per-user store root, not beside the checkout, so settings
do not depend on which directory the app was launched from (a cwd-relative
`autoshade.local.json` is still read as a legacy fallback).
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
| M4 | Style retrieval + eval harness (your edits as ground truth) | k-NN over EXIF+histogram, plus an optional SigLIP 2 cosine term (`AUTOSHADE_STYLE_EMBED`, off by default); per-field MAE/bias | **done** |
| M5 | Local web UI | `tiny_http` + vanilla JS (gallery, live before/after) | **done** |
| V2 | AI denoise (high-ISO/astro) | Python sidecar → **SCUNet** on GPU, called from Rust | **done** |
| V2 | Baked-source mode (edit exported PNG/TIFF) | extension dispatch; develop runs on loaded pixels | **done** |
| V2 | Generative reimagine / retouch | OpenAI Images (`gpt-image-*`); reimagine composes a faithfulness scaffold onto the prompt under `high` (the `input_fidelity` parameter is negotiated away on gpt-image-2), measures the result's structural divergence D with the reverse-fit's own statistic (`fit::structure_divergence_for`, threshold `fit::DIVERGENCE_GLOBAL`), disclosures it, and offers a bounded opt-in retry that keeps the closer of two results | **done (experimental)** |
| V2 | Pixel retouch / heal (spot removal) | deterministic heal engine + vision spot-detect ([`src/retouch.rs`](../src/retouch.rs)) | **done (experimental)** |
| V2 | Look matching / reverse-fit (`match`) | distribution-level solve for the recipe that reproduces a target rendition ([`src/fit.rs`](../src/fit.rs); zoned variant [`src/fit_zoned.rs`](../src/fit_zoned.rs), range layer [`src/fit_zoned/range.rs`](../src/fit_zoned/range.rs)) | **done** |
| V2 | Cross-image correspondence (content-divergent pairs) | DIFT (SD 2.1) sidecar → 48×48 field of target coordinates + cyclic×smoothness confidences ([`src/correspond.rs`](../src/correspond.rs), [`python/correspond.py`](../python/correspond.py)); CLI `correspond` diagnostic; the reverse-fit consults it automatically on content-divergent pairs (the fit's own D gate, single-sourced) and FULL zone fits weight pairs by confidence + read shifted content at its corresponded position — share gates and Atmosphere zones keep pre-field semantics, and identity/zero fields are conservation-tested to change nothing | **done (7a instrument + 7b estimator wiring)** |

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
| Embedded preview at an **in-camera aspect crop** (a 4:3 preview over a 3:2 sensor) | **Treat it as the crop it is** (v1.2.2): `reimagine` sizes and sends the sensor frame, `match` fits on a neutral develop of the sensor frame with the calibration composed, the base-look estimator pairs the develop's centred crop | The preview is a display artefact; every consumer that took it for the frame paired two different frames (`fit::same_frame_plausible_dims` is the one rule) |
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
sensor corner (v0.32.0).** Block registration of eight AutoShade renders against
their Lightroom exports put every one of them **(+31 ± 6, +20 ± 1)**
full-resolution pixels off, a pure translation with no scale component. The
ARWs carry `DefaultCropOrigin = (32, 20)`, `DefaultCropSize = (9504, 6336)`
inside a `9600 × 6376` raw frame: AutoShade emitted the right SIZE from the
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
current-frame at their boundary instead. Raster (`MaskGeometry::Bitmap`) masks
are image files, not coordinates: they are left alone and the user is told so
(`render::recipe_has_raster_masks`, whose one member they now are). Imported
BRUSH groups used to join them on that side of the line for a different reason —
their dab coordinates were carried verbatim so the sidecar round-tripped
byte-faithfully — and R29 C1 moved them off it: the brush RENDERS since R29
Batch-6b, so a verbatim carry meant a mask drawn a quarter turn away from every
parametric shape beside it. The migration now rewrites the stream numerically
(`d` through `orient_point`, `r` and `crs:Radius` rescaled by W/H, because a dab
is a circle in PIXELS while the radius is in width units — that aspect is
`render::CoordFrame`, the input the function had to be given). The accepted cost
is stated where it is paid: a rotated — or portrait — photo's republished dab
stream is no longer byte-identical to Lightroom's, only numerically equal on
Lightroom's own six-decimal grid. An unrotated landscape photo is untouched.
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
(`render::texture_pass`) is a small-radius detail operator placed between
clarity and saturation for ACR's Basic-panel order and called by the global
stage and the mask arm alike — so 「Texture +30」 means the same structure
globally and inside a mask. **Its two halves are not the same operator.**
Positive is the plain `unsharp_luma` of clarity at 0.005·min(w,h) (floor 2),
measured against Lightroom in R27 and untouched since. Negative is **fitted to
Lightroom, since R29 Batch-8-2** (`render::texture_negative_pass` — two
controlled ladders, the second on a clean base after the first turned out to
have been shot with capture sharpening at maximum):

```text
l' = l − w(t)·[ A₁·(l − G_σ₁∗l) + A₂·(l − G_σ₂∗l) ],  t = |slider|/100
A₁ = 0.172443  σ₁ = 0.0031235·min(w,h)      w(t) = t(1+d)/(1+d·t)
A₂ = 0.304888  σ₂ = 0.0002822·min(w,h)      d    = 0.558583
```

Two low-passes mixed in **parallel** (not cascaded), scaled by a hyperbolic
depth law. The shape is a monotone **high shelf** — at −100 a 256 px tone keeps
0.992 and the finest scales settle on the plateau 1 − (A₁+A₂) = 0.523 — and 45
acceptance anchors (nine periods × five ladder steps, b8-analysis-2 §6-3) are
pinned as a test at ±0.02, where the shipped kernels measure 0.0037. σ binds to
the **render raster's** short edge, adjudicated at 16× separation by a
two-resolution export pair; this engine develops at full resolution and resizes
last (`main.rs:891-894`), so that is already what `texture_pass` is handed. The
fit only holds in the sRGB-gamma domain, which is where the develop buffer
already lives. **This IS a rendering change for every negative texture value,
global and per mask** — the second such change on this branch: v0.34.0 replaced
the pre-R28 full Gaussian blur (whose endpoint erased fine detail, σ −92 % in
the visual-inspection package) with a band-limited **notch**, and R29 B8-2
refuted the notch's shape outright — Lightroom takes MOST out of the finest
scales, where the notch kept 0.9992 of a 4 px pattern against Lightroom's 0.57.
Two honest limits stay registered rather than papered over: Lightroom's operator
is amplitude-adaptive (not LTI), so a fixed kernel matches the *ensemble*; and
on the 1280 px GUI preview σ₂ falls to 0.241 px, so that arm is **clamped off**
and the preview's negative texture is weaker than the export's by up to 0.076 in
transfer (user ruling: clamp and disclose). The XMP still carries the raw slider
so Lightroom re-renders it with its own model. **Manual CA** (`ca_r`/`ca_b`) folds into the
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

Since v0.13.0 AutoShade does **not** write it next to the photo: the source
library is read-only, so the projection lands in the per-user develop store
(`<AUTOSHADE_DATA_DIR | %LOCALAPPDATA%/autoshade |
$HOME/Library/Application Support/AutoShade | $XDG_DATA_HOME/autoshade |
$HOME/.local/share/autoshade>/develops/<stem>-<hash of the absolute
path>/<stem>.xmp` — see `store::store_root_with_trust`, and the trust bullet in
§3 for why the shared-temp last resort is labelled rather than
trusted; and `store::adopt_pre_rename_root` for the one-time adoption of a
pre-rename `autoshop` store — a single same-volume `rename`, never a copy,
because a copy that dies half-way through gigabytes of mask rasters leaves two
divergent stores with no way to tell which is real. Both names present means
the current one is used and the old one is left untouched: merging two stores
decides which of two develops of the same photo the user meant to keep. A
failed rename keeps the OLD folder in use, so the edits stay reachable),
alongside `recipe.json` (the authoritative develop
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

One GUI `Ctrl+S` saves every variant card: the active card's develop and pixel
origin go to `recipe.json` / `pixels.json`, and every other card goes to the
same generation's `variants.json.others`. Unsaved-state checks resolve each
live card against that persisted union by stable ID first, with kind + strip
position only for an id-less legacy side. Changing `active_kind`, `active_pos`,
or `active_id` merely by viewing another card is navigation, not an edit; the
selection is persisted only by the next save, so reopening lands on the last
SAVED active card rather than the last viewed one. Card names remain saved
strip state and therefore remain unsaved work when changed.

On the strip itself the ACTIVE card carries the actions that act on the live
canvas: 「＋」 snapshots THIS card's develop only as a numbered version
(`v<n>.recipe.json` + frozen mask rasters + `.version-meta.json`; v0.30.0), the
card's name is editable in place (the rename buffer is keyed by the card's own id, so
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
file we did not author (all 1081 mask components in the 177-sidecar reference
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
by file. (Re-measured through the `AUTOSHADE_MB_FIXTURES` probe as the round went
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
token stream, the stream carried VERBATIM as a string (verbatim until a TURN
touches it — R29 C1 rewrites the numbers so the mask renders in the right place;
see §「the `coord_era` migration」). It is parsed, kept in
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
(「brush mask(s) drawn from AutoShade's measured model of Lightroom's brush -
not Adobe's own rasteriser」), because the edges are ours and not Adobe's — the
same shape of statement the AI-mask arm makes, one notch weaker because ours
came from a measurement of Adobe's own output. Measured on the specimen
folder that has brush work in it: `P12` went from 8 of 11 corrections
imported to 10 of 11, and the one still refused is refused for
`CorrectionAmount="1.1"`, not for its brush.

~~**The measurement then LANDED, and the arm still does not render (R27
Batches 8-10).**~~ **← HISTORICAL from here to the end of this block: it
records the R27 state, and both of the blockers it names fell in R29 (the
paragraph above is the shipped behaviour). Kept because the measurements are
still the provenance of the model that ships, and because the two blockers are
worth being able to look up. Read every present tense below as「in R27」.**
Two controlled Lightroom experiments — 16 hand-made exports,
then 17 states written as SYNTHESIZED sidecars that Lightroom imported and
rendered without complaint — close the alpha model except at one axis:
accumulation is **screen**, within a stroke and across components (a held-out
51-dab stroke predicts at rms 0.0045); **density (`MaskValue`) scales each dab
BEFORE the screen** rather than capping it (the cap reading is refuted 13×);
and **flow** obeys a one-parameter odds law, `D/(1−D) = κ·f/(1−f)` with
~~**κ = 0.1219 ± 0.0027**~~ (fit rms 0.0070 over 11 rungs, held-out 0.0097, and
`D(1) = 1` exactly with no free parameter — which killed the earlier
「flow 1.0 takes a different code path」 hypothesis). **← the SHIPPED value is
R29 B6's `κ = 0.1284 ± 0.0029`** (`render::KAPPA`), re-measured out of sample
and universal to 2.24 % across a 3× radius change and both hardness ends. The
R27 number sits 5.3 % low, ≈ 1.6 σ — real, and not to be quoted as agreeing
with the shipped one better than 5 %; the odds law carries a genuine ~11 %
per-rung κ drift across flow, which is registered rather than fitted away.
Two identities fell out:
Lightroom's brush **Size is the α = 0.5 diameter** (266.1 ± 5.0 px, invariant
across the feather ladder) while `crs:Radius` is the OUTER support, and
`CenterWeight ≠ 1 − Feather/100` (Feather 50 → 0.1621).

Two named reasons kept the renderer at weight 0 through R27, **and BOTH are
closed** — the strikethroughs below are what changed, not the record.
~~**(1) The kernel has no closed form.**~~ `k(ρ;h)` is measured at 11 hardness
rungs; six families were tried and the only one spanning h = 0 → 1 has
parameters DISCONTINUOUS in h, so what exists is a measured TABLE whose
h-interpolation predicts a held-out rung at rms 0.0115 and max **0.0344** — 4×
the 0.0085 quantisation floor. **← R29 B6 found the closed form on denser
sampling: `k(ρ;h) = (1 − ρ^m(h))^n(h)` with `ln m` and `ln n` cubic in the
hardness — 8 numbers, held-out rms 0.0109, BETTER than interpolating that table
(0.0180). The table was the measurement; the closed form is what ships.**
~~**(2) The mask does not live in the frame this engine renders in**~~:
Lightroom rasterises
it BEFORE its lens correction (the same artefact the `k = 1.032` bullet below
closes), displacing exported dabs by up to 57 px and stretching them 7.4 %
anisotropically at the frame corners — ~~and this engine has no `.lcp` parser,
never reads `crs:LensProfileEnable`, and runs Sony EXIF knots in its own
geometry stage, a DIFFERENT polynomial~~. **← R29 Batch-3 (`src/lcp.rs`) both
built the `.lcp` reader and read `crs:LensProfileEnable` — and for BRUSHES the
frame half turned out to need nothing at all: this engine evaluates masks
BEFORE its own geometry stage, which IS Lightroom's pre-correction frame, so a
dab stream is already in the right place and gets no warp. The parametric
shapes are the ones that needed the reader; see the mask-frame block below.**
~~Baking a mask into pixels at a position
known to be wrong is worse than the honest `BrushCarried` disclosure, so the
implementation waits: the sketch is on file (`batch10-report.md` §7.4 —
pre-rasterise the dab group and sample it exactly like `Bitmap`/`AiMask`, no
schema change, with κ and the 11-rung table as the pinned test values), and the
frame half of the blocker is what an `.lcp` reader would answer (the named R28
candidate).~~ **← the sketch was followed in R29 B6b, and the disclosure was
renamed with the behaviour: `BrushCarried` no longer exists in the tree — it is
`BrushRendered` in both directions.**

Reading a Paint required three parser fixes in the same batch, all of them
latent-until-now: `classify_correction` walked the component list FLAT (so a
stroke inside a group read as a sibling of it), `base_geometry_at` searched the
whole correction segment nesting-blind (so a gradient nested in a group could
have been promoted to the correction's base shape), and `parse_one_correction`
read its geometry keys from a slice running to the END of the correction (so a
later component's `MaskValue` could answer for a base that omitted its own).
All three are nesting-aware now, through one shared `components_in` walk.

`Mask/Image` — the AI subject/sky/object masks — was the other half, and R27
Batch-5 took it. Those files carry no raster: only the INTENT (`MaskSubType`,
`MaskName`, `ReferencePoint`), optional gesture region hints, the provenance
digests, and the proxy geometry the model ran in. So reproducing one was never
a parser question: it needs a segmenter of our own producing our own alpha,
which is a DIFFERENT feature — `MaskGeometry::AiMask` carries the intent and
the 11 whitelisted provenance attributes verbatim, `segment::resolve_ai_masks`
recomputes the alpha at develop time and caches it, and subtype-0 gestures add
their ordered positive dab points after the ReferencePoint. Every surface calls
the result a re-derivation (see 「AI masks are a RECOMPUTATION」 above). With that arm the
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
AutoShade no longer propagates the deletion to the sidecar (delete them on the
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
it at the pixel (`P24` 8.3 : 1 at +24.35°; `P22` decoded tilt
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
  off) and **Lightroom rasterises its masks BEFORE the lens correction**, ~~a
  frame this engine cannot reproduce today — no `.lcp` parser,
  `crs:LensProfileEnable` never read, and its own geometry stage runs Sony EXIF
  knots, a different polynomial~~. Rendering the geometry the sidecar actually
  stores and leaving Adobe's warp UNMODELLED is what the user ruled; ~~an `.lcp`
  reader is the named R28 candidate~~. The `k` plumbing stays (it is the shape a
  warp model slots into) and is now the identity everywhere. **What this
  costs and what it does not:** the byte round-trips were `k`-invariant by
  construction, so the real-sidecar suites pass unchanged; what moves is the
  RENDER — an imported radial is no longer dilated 3.2 %, and the residual on
  any frame is that frame's own Adobe warp (0–3.4 % observed).
* **The mask frame is a per-mask-TYPE and per-topology decision — R29 Batch-3,
  D2 `706ac84`, and D2 LINEAR `ad6de62`.** The `.lcp` reader
  ([`src/lcp.rs`](../src/lcp.rs)) and camera metadata solve the Lightroom map;
  `MaskWarpSource` names which of seven provenance/refusal states applies.
  Sony's 16 private distortion samples are interpreted on their measured native
  radii `(i+1)/16`, resampled to 2048 canonical nodes for the mask solve, and
  emitted as a 64-knot `mask_warp`. Camera-metadata profiles also persist
  `mask_warp_center = raw_full_dims/2 − DefaultCropOrigin`, rather than assuming
  the stored-frame centre. The ordinary render spline keeps its established
  calibration because the image-registration gate rejected changing it.

  The resulting frame table is exact about topology:

  | Mask | Downstream geometry | Frame operation |
  |---|---|---|
  | Brush / bitmap / AI | either | identity |
  | RADIAL | active | sample through `m_lr⁻¹ ∘ T_engine` exactly once |
  | RADIAL | inactive | identity at stored coordinates |
  | LINEAR | active | sample at `T_engine(p)` only; rebuild the straight line in the corrected frame |
  | LINEAR | inactive | map Zero/Full once through `D_fwd`; rebuild one straight line in the raw pixel metric |

  `MaskFrame` makes the caller's downstream fact explicit with three states:
  `WarpedDownstream`, `LinearHandlesToRaw`, and `AsRendered`. The disabled-
  sidecar case keeps RADIAL identity in `mask_warp` while retaining LINEAR's
  handle map separately in `linear_handle_warp`; every other solved state uses
  `mask_warp` as the handle source. Pixel loops never call the handle map.

  Radial transport closes all 41 measured point vectors to ≤1 px (wall 20/20
  RMS 0.568 px; second set 21/21 RMS 0.243 px). Clean dilation is ≤0.35 pp and
  R1 is about 0.5 pp; the R2 big-mask excess remains open at about 1.2 pp.
  LINEAR H2 is intentionally not described as 1 px-closed: ON residuals are
  9.748/7.025/6.336 px RMS and OFF residuals are 12.449/9.943/4.979 px RMS.
  A fitted anisotropic-aspect candidate is diagnostic only and is not shipped.

  Linear coverage has one engine law, `linear_coverage(t, profile)`, applied
  after the existing H2 handle transport and pixel/aspect projection. The
  shipped `LINEAR_FALLOFF` is `Eased`, the measured C1 Hermite smoothstep:
  Lightroom turns over at both handles (80/80 rows) and a free-end profile
  fit (handle rows and profile fitted jointly, so a soft profile cannot hide
  inside a shrunken span; `scripts/linear_falloff_probe.py --fit`) reaches RMS
  0.0045 for smoothstep against 0.017 for linear. This is a render hard change
  for linear
  masks only; radial and bitmap masks remain byte-identical.

  ⚠ `mask_warp_center` and `linear_handle_warp` are two deliberate v1.0.0
  **hard forward schema breaks** inside `LensProfile`: older `deny_unknown_fields`
  readers refuse a recipe carrying either fact instead of silently dropping a
  coordinate map. Old recipes default both fields and remain readable.
* **A mask is sampled at PIXEL CENTRES — R29 C2, v0.35.0.** `render::
  MASK_SAMPLE_CENTRE` is the one constant behind five sites that must agree
  (`apply_masks`' frame producer, `mask_coverage`'s overlay, `sample_gray_norm`'s
  texel lookup, `rasterise_brush_group`'s stamp grid, and `fit_zoned`'s analysis
  moments). Measured on two different negatives: a hard-edged radial whose
  nominal centre is the continuous `(3120.0, 2080.0)` fits at `(3119.46,
  2079.50)` and `(3119.49, 2079.51)` in PIXEL-INDEX space, and an ellipse fitted
  over indices returns `p − 0.5` — so Lightroom maps a stored fraction `u` to
  `u·W` and pixel `i` carries the value at `u = (i + 0.5)/W`. The old `x/w` gave
  pixel `x` the value belonging to its own top-left CORNER. ⚠ Every mask of
  every family therefore lands half a pixel up and to the left of where this
  engine used to put it — exactly 0.5 px on each axis, with no dependence on
  feather, geometry or frame size. No calibration needed compensating: the
  falloff table's ρ is normalised against a fitted centre and semi-axes
  (convention-neutral), the brush kernel's radial profile moves by O((δ/r)²),
  and the texture anchors are a spatial filter with no mask coordinate in them.
* **the falloff** — three successive closed forms, all refuted, now replaced by
  the measurement itself. Since **v0.35.0** the `MaskGeometry::Radial` arm of
  `mask_weight` calls `render::radial_falloff`, which interpolates Lightroom's
  measured α(ρ) out of a table: rows are the measurement's own ρ bins
  (`0.0025 + 0.005 i`, 290 of them), columns are `Feather` 1 / 5 / 10 / 15 / 25
  / 35 / 50 / 65 / 75 / 90 / 100, and `Feather = 0` is an analytic hard edge
  rather than a measured column. That table and the reasoning behind every
  choice in it live in `radial_falloff`'s own doc comment; this is the summary
  of how it was arrived at.

  v0.32.0 landed `ramp(1 − f, 1 + f/2, d)`: a cubic smoothstep from
  `d_in = 1−f` to `d_out = 1+f/2`, fitted on an 11-rung exposure ladder across
  five frames (aspect 1.03 … 7.46, one rotated, one corner-placed). The engine's
  outer edge had been at `d = 1`; the measured one at `Feather = 50` is 1.25,
  i.e. **the mask was 29 % under-sized** and no amount of correct geometry
  upstream could recover it. That much survives — the outer boundary does move
  with feather. **R27 Batch-10 then refuted both endpoints**: an untouched
  reference export unlocked the inner branch and `d_in` read 0.558 / 0.348 /
  0.041 / **−0.144** at `Feather` 25/50/75/100 against the law's
  0.75/0.50/0.25/0.00, while `d_out` appeared to SATURATE near 1.41 instead of
  climbing to 1.5.

  **R29 Batch-7 found out why, and Batch-7-2 closed it.** Both readings were
  artefacts of forcing ONE smoothstep across a profile that is not one: with the
  nomask reference the earlier batches lacked, `d_out` is CONSTANT in feather
  and α(0) = 1 at EVERY feather (mask centres pixel-identical to the feather-0
  frame on all eight rungs), so there is no inner knee to fit. No two-parameter
  closed form reaches the 0.003 measurement floor — the best of five families is
  3.1× it, the shipped smoothstep 4.5× — and the one candidate law the four-rung
  batch had spotted, `a ≈ 1.9/f`, misses by 58× at `Feather = 1`. So the table
  IS the model. Scored on the batch's own grid it reads rms(α) ≤ 0.0009 on every
  rung against the old law's 0.0093 … 0.1557, and puts the α ≥ 0.5 area ratio at
  1.000 against 1.105 … 2.077.

  Better on every rung was the requirement, not a bonus: the old law was already
  CORRECT for `Feather ≤ 5`, so the table degenerates to an exact hard edge as
  feather → 0 rather than to a table row, and the narrow rungs are pinned as
  absolute numbers in `the_radial_falloff_beats_the_refuted_ramp_on_every_feather`.

  **R29 me3 then tightened it twice over.** Three more controlled exports
  measured `Feather` 15 / 35 / 65 — the gaps the eight-column table had to
  interpolate across — and it was reading 14.8 px and 24.9 px wide on the
  α = 0.5 contour at 15 and 35. Those three columns are INSERTED (the eight
  earlier ones carry over bit for bit, max |Δ| = 0.000000), because changing the
  interpolation family instead buys 1.5× where carrying the measurement buys the
  whole residual. Two registered residuals close with them: `d_out` is **√2**,
  with 1.43 and B7's 1.4335 excluded by four shape-free instruments and by a
  forward check that puts their predicted endpoints past what the pixels show
  (the table never states `d_out`, so this costs zero pixels), and aspect
  invariance is sampled once — the shipped table scores rms(α) 0.0009 on a
  held-out aspect 1.2 export against 0.0004 on the fitted 2.5, with the best
  single radial rescale between the two at k = 1.00076, i.e. nothing in the
  falloff is anchored in pixels. Still registered: that check is one extra
  aspect at `Feather = 50` only, the `Feather = 1` far tail is unresolved, and
  BETWEEN columns is still unmeasured at the `Feather ≤ 10` end.
  (`~/.claude/plans/r29-materials/b7-analysis.md`, `…-2.md`, `me3-a-report.md`
  and `me3-b-report.md`; the item lives in
  V2_PLAN §7 item 1 — M1_PLAN and V2_PLAN are development ledgers kept outside
  the public tree since 2026-08-20, the same standing as the probe reports these
  sections cite.)
* **`crs:LocalHue`'s scale is 180, not 100.** A controlled export with the mask
  Hue slider at +50 wrote `crs:LocalHue="0.277778"`; 0.277778 × 180 = 50.00004.

~~`W, H` are the exported pixel dimensions (= `DefaultCropSize`)~~ **ERRATUM,
R27 Batch-3:** `W, H` are the **un-rotated SOURCE frame** — `DefaultCropSize`,
which equals the exported dimensions only while `HasCrop="False"` and the
capture is landscape. The two readings diverge the moment a crop exists
(`P5-cropped-mask-frame.md` §1: reading a cropped export's own dimensions
displaces `P32_16.9.JPG`'s five radials by 834–1384 px) and again on a
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
bullet above. **v0.35.0 moves it a THIRD time**, back to the falloff and this
time to the measured table: every radial carrying `Feather ≥ 10` re-renders,
`Feather ≤ 5` moves by the old law's own residual there, and `Feather = 0` is
byte-identical because it takes the analytic branch.)

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
`autoshade style-index <dir>`, by the web info panel, and — since R23-2 — by the
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

**ONE RAW/XMP pairing rule ([`src/xmp_pair.rs`](../src/xmp_pair.rs)).** The rule
used to be open-coded as `raw.with_extension("xmp")` at six sites — the
style-index pair scan and its sidecar read, `eval`'s pair scan, its per-photo
read and its resume hash, and the CLI's Lightroom import note — with two
consequences. Sidecars kept in a SEPARATE tree could not enter the index at
all; and `with_extension` writes a lowercase `.xmp`, which `Path::exists` folds
on Windows but not on macOS or Linux, so the same library produced different
pair counts on the Windows and Mac builds and said nothing about it.
`XmpPairing::find` is now the only definition: given the library root and an
optional `--xmp-dir`, it looks in the mirror of the library tree
(`<xmp-dir>/<relative folder>/<stem>.xmp`), then the flat mirror
(`<xmp-dir>/<stem>.xmp`), then beside the RAW, and it matches the EXTENSION
case-insensitively on every platform by LISTING the folder rather than probing
spellings. The STEM is matched exactly: every writer of a sidecar reproduces the
photograph's name byte for byte, and folding it would let one RAW claim
another's edit on a case-sensitive volume. Listings are memoised per folder, so
a 2,000-photograph library pays one `read_dir` per folder rather than per
photograph. The module is READ-ONLY by contract — nothing is ever written to a
path it returns — which is why the develop chain's own beside-the-RAW sites that
are also a WRITE target (`store::xmp_beside_target`, and the restore ranking
around it) deliberately stay on the old spelling; `pipeline::write_xmp`'s merge
base, which only reads, moved with the note that discloses it. The build now
also discloses the RAWs it could not pair (count, first ten stems), so "you
pointed me at the wrong folder" no longer reads as "you have edited 40
photographs".

**Incremental builds: one content-keyed cache mechanism, two caches.** Every
build used to redo the decode, the 14-dim feature, the SigLIP image vector, the
vocabulary scores and the SigLIP text vector for every photograph; only the Qwen
description survived, in `describe::DescriptionCache`. The file mechanics of a
content-keyed cache — the byte cap, the four degradations (absent / over-cap /
not UTF-8 / unparseable ⇒ an EMPTY cache and one sentence, never an error), the
`tmp` + `durable_replace` publish, the 64-hex key rule — now live once in
[`src/content_cache.rs`](../src/content_cache.rs), and
[`src/style_cache.rs`](../src/style_cache.rs) is the second user:
`<store>/style-exemplars.json`, keyed by the **SHA-256 of the staged frame**
(`describe::frame_digest`), the same key shape the description cache has always
used. Each entry also records a `SourceStamp` — absolute path (case-folded on
Windows), length, mtime and the photographer's saved quarter-turns — because
the frame digest cannot be known without the decode that produces the frame, and
the 14 features are a function of the FILE (EXIF + histogram + rotation) rather
than of the frame. A build that finds an exact stamp match may reuse the entry
whole and never open the RAW; anything else decodes, stages, hashes, and still
reuses the model answers under the digest. That fast path is ALL-OR-NOTHING
against the passes the build asked for (`cache_answers_everything`): a record
that skipped its decode has no staged frame and so could never afterwards be
embedded or described. Entries are gated per field — the index feature version,
`embed_provenance_string()` for the vectors, and
`describe::CachedDescription::is_current()` for the prose — and admitted at load
only inside the INDEX door's own bands (`CACHE_BANDS`, from
`exemplar_is_finite`), because a bit-rotted vector served into a published index
would make that index refuse to load. What is never cached is the
NORMALISATION: `compute_norm` takes the mean and σ over the whole exemplar set,
so one photograph joining or leaving legitimately moves every z-scored
dimension, and both are recomputed from the merged set on every build. The
`.xmp` is re-read every build too — sliders, curve, colour families and mask
habit are properties of the sidecar, not of the pixels. `CURRENT_INDEX_VERSION`
does NOT bump for this: no field is added to the serialised index, nothing about
the fourteen features or the ranking changes, an old index loads exactly as
before, and the version that gates reuse is stamped inside each CACHE entry, so
a future bump silently invalidates the cache instead of misreading it. Each
build prints `style index cache: reused N, recomputed M, removed K,
skipped-for-sidecar S`; a full-hit rebuild loads neither model checkpoint. The
LOOK library keeps the description cache and nothing more — its records carry no
14-dim feature for an entry to be about, it is capped at 500 curated finished
photos, and it is rebuilt only when that folder is re-curated.

The index door has ONE table for the `settings` labels it admits —
`style::setting_bands`, the recipe's own clamp bands, read by `load` for every
stored exemplar. `read_settings` writes the twelve reference sliders and the
distillation vocabulary v1.2.0 added (the 24 HSL cells and the 14 colour-grade
fields, `distil_keys`); until v1.2.2 the loader carried its own twelve-label
list, so a library with one HSL or colour-grade edit produced an index the same
binary refused to read back ("exemplar 0 has an unsupported setting key"), and
`style-index --looks`, which merges into the existing file, replaced it with a
looks-only index. Two tests hold the two sides together: every label the writer
can produce has a band, and an exemplar carrying all fifty survives
`save` → `load` clamped to the recipe's ranges. A label nobody writes is still
refused at the door.

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

**S1 continuation: text and finished-look retrieval.** The RAW index keeps the
14-dimensional EXIF/histogram retrieval it shipped at v5, is written at version
6, and may carry additive
SigLIP 2 image vectors, Direction text vectors, zero-shot vocabulary scores,
bounded tags, and an optional description vector. Its distance is
`d14 + W_EMB(1-cos(q_img,e_img)) + W_TXT(1-cos(q_txt,e_img)) + W_DESC(1-cos(q_txt,e_desc))`;
missing or width-mismatched vectors contribute zero, and zero weights preserve
the v5 order bit for bit. The vocabulary is a versioned, grouped list of 33
SigLIP-style photographic phrases; at most one winning phrase per group enters
the four-tag summary, and a group's winner is the phrase whose score stands
furthest above the LIBRARY's own mean for it, so a caption every photograph
scores highly on cannot own a tag everywhere. That is what version 6 is: tags
feed `desc_text`, so re-deriving them moves the ranking and not just the
printed summary. Every index this build reads — v4, v5 or v6 — therefore has
its tags re-derived on load rather than trusted from disk.

**S3: the local-work habit.** An exemplar may additionally carry a
`MaskHabit` (`src/mask_habit.rs`) read from the same sidecar the settings come
from, through the develop chain's own path-aware importer
(`xmp::xmp_to_recipe_for_photo`): the number of masks the photographer ENABLED
with a non-zero amount, how many carry a Range Mask refinement (counted from the
imported recipe AND from the import's own `ForeignRangeMask` refusals, because a
Range Mask this engine cannot honour is dropped on the way in — twelve are
refused and none carried on the calibration library), and per USE
(`sky` / `subject` / `ground` / `range` / `other`) a count plus the
amount-weighted mean of ten local sliders (in-mask temperature and tint
included) plus the share of uses carrying a local point curve. The use is
decided by one pure
function, `bucket_of` — an AI selection answers from its own `MaskSubType`, a
linear gradient from which end of the y-down frame its `full` handle covers
(XORed with `MaskInverted`, which reverses the covered end), a radial from
whether it is inverted, and a Range Mask only when the geometry says nothing,
so a refined sky gradient stays a sky gradient. **No geometry is ever averaged**:
a mask's coordinates are a fact about one frame, so the block carries counts,
slider means and English, never a coordinate. Retrieval does not read the field
at all, which is why it ships WITHOUT an index-version bump on `families`'
precedent — `#[serde(default)]` in both directions, so a pre-S3 index loads with
the field absent (a different fact from a measured zero) and a pre-S3 build
ignores the new key. `style_targets`/`blend_toward` DO read it: a mask's slider
AMPLITUDES are one of the five distillation channels, gated per (bucket, slider)
by the same consistency rule as every other channel, and nothing spatial is read
or written — the proposer still places every mask itself. The build prints
one aggregate line for what it learned and one for the mask content
`xmp::import_losses` says it could not carry whole; `style-query` prints the
per-neighbour counts.

**The block's own budget.** Everything an index carries is bounded for STORAGE
(`MAX_DESC_CHARS` 512 per description, 128 per tag), and a prompt block is a
different budget: `advisor::REFERENCE_BUDGET_BYTES` (4,096) with
`BoundedUntrustedText` cutting whatever overflows. Four neighbours at their
storage bounds measured 5,920 B — 45 % over — so the tail was being cut, and
since S3 that tail is the local-work note. The block now has its own door
(user ruling 2026-08-30): `REFERENCE_DESC_CHARS` 200 per description,
`REFERENCE_TAG_PHRASE_CHARS` 48 per tag phrase and `REFERENCE_TAGS_CHARS` 128
per joined tag list, applied at all three tag consumers (the per-exemplar look
note, the shared-tag note and the look-reference block). The index, the text
tower and `style-query` still see the whole sentence; only the block is cut,
and it says so with an ellipsis. Measured: the widest block this app can build
is 3,565 B with the note, a realistic one 2,517 B
(`style::tests::the_local_work_note_fits_the_proposers_budget` prints both).

The two direction-text terms carry a **standardised variant** (z-score over the
candidate set, with a disclosed raw fallback below three comparable candidates
or a degenerate spread), built because raw SigLIP image↔text cosines are tightly
clustered. Since the retrieval-rank batch that standardisation is TWO centrings:
the direction-text ↔ exemplar-IMAGE term first has each candidate's **text
hubness** removed — its mean cosine against the whole 33-phrase vocabulary, which
every embedded record already stores as `vocab_scores`. Centring over the
candidate set only takes out the level the PHRASE sits at; the photographs that
score high against *every* sentence survived it, and on the user's 169-exemplar
index that per-candidate constant is 21.7 % of the cosine's variance against
25.3 % for the direction × candidate interaction that is the only part able to
tell two directions apart. It is ALL-OR-NOTHING over the candidate set (a
corrected candidate must never be ranked against an uncorrected one) and the
terms disclose which happened; the description term keeps no correction, because
the only available bank made antonym pairs agree *more*. S2's calibration harness measured both variants over the whole grid
under BOTH query-text proxies: under the tag-string proxy the raw variant
wins and neither text term can be told apart from zero, while under the
real-prose proxy the standardised variant beats its own text-free row with a
paired CI excluding 0. So the standardised arm is the only one that ships and
the raw path is gone rather than kept one flag away: a switch nothing flips is
an untested second ranking carrying weights nobody calibrated for it, while the
harness still sweeps both arms and prints both tables. A third proxy of typed
SHORT directions then settled the head-to-head S2 could not — best raw 0.410916
against best standardised 0.377820, paired CI [+0.021870, +0.043719] — and
priced the shipped weights on the same run: `W_TXT` 0.5 costs nothing
measurable (+0.000447, CI [-0.006972, +0.007980]) and is the largest weight
that leaves the corpus open, while `W_DESC` 0.5 costs +0.013643 (CI
[+0.006785, +0.020881]) and buys the antonym separation that is the Direction
control's only other mechanism (top-1 overlap 46.9 % with it, 60.7 % without).
The switch and the
four weights are resolved once, as values (`EmbeddingSwitch`,
`RetrievalWeights`), and travel on the develop request; nothing writes the
process environment to express a flag, and `retrieve_with_embed`,
`distance_components` and `retrieve_looks` all read one scoring helper, so the
diagnostic prints the numbers the ranking used rather than a second
implementation of them.

Finished baked photos are stored in a separate look-library block with their
own source directory and no camera features or develop settings, capped at 500
records (the file holds both populations, so both are capped against one
envelope). Look retrieval uses image/text/description cosine terms only, with
`W_LOOK=1.0`; it can guide the proposer and optionally supply the reference
image, but it never reaches `style_targets` or `blend_toward`. A look image
that fails to load falls back to the RAW neighbour and discloses both. With
embedding disabled or no query vector, the look answer is unreachable and the
rationale states that fact. The look block and the IMAGE 2 sentence claim the
direction helped choose the look only when a text weight is actually non-zero.
Direction adherence is an independent Hint/Direct/Brief prompt tier using the
same band edges as Strength; the verifier is told the selected tier, and only
when a direction exists.

**Who leads when a direction is given (v1.2.3).** Until v1.2.3 the retrieved
library was always the thing to hit: the block said either "stay within it" or
"REPRODUCE this look", and `blend_toward` then lerped the finished proposal onto
the neighbours' means. A free-text direction could only move the proposal WITHIN
that. Measured on the island showcase frame (2026-09-01, `--style 1.0 --strength
0.9`, an index of 169 RAW+XMP exemplars plus 94 finished looks): three directions
as far apart as *dark moody low-key … teal-and-orange*, *warm golden tones,
film-like grain, lifted matte shadows* and *vivid saturated colours, punchy high
contrast, crisp clarity* developed to per-panel-cell mean HSV S/V of **23/54 ·
11/58 · 17/55** — all three inside the library's own cool hazy register, against
the neutral develop's 17/47. The same three directions against an index with the
photographer's own edits removed separated to **34/38 · 12/61 · 29/65**.

The user's ruling: when a direction is given, the photographer's own edits are
background and the direction leads. The mechanism is the adherence dial the app
already had, not a fourth control. `style::StyleVoice::choose(style, direction,
adherence)` is the ONE derivation — `Background` when a non-empty direction meets
tier `Direct` or `Brief`, otherwise the historical `Ceiling`/`Target` split at
Style 0.85 — and it decides two things at once:

* **the wording.** `render_reference_voiced` renders the block in that voice. In
  `Background` the header reads "STYLE BACKGROUND … The DIRECTION LEADS" and each
  of the four aim clauses (curve, colour families, shared look, local work) becomes
  a habit the direction may override. The measured NUMBERS are byte-identical
  across all three voices; only the aims move.
* **the arithmetic.** `pipeline::style_blend_pull` returns `None` in `Background`,
  so neither the verified proposal nor a judge candidate is pulled toward
  `style_targets`, and the re-verification that follows a real blend is skipped
  with it. Wording alone would have been a lie by arithmetic: at Style 1.0
  `style_pull` is FULL, i.e. the proposal's own value is replaced. Both call sites
  are pinned by a source guard
  (`pipeline::every_blend_site_in_this_file_is_guarded_by_the_style_voice`), because
  no test in the battery can run the blend path itself — it needs a paid analysis —
  and the judge-candidate site is the one that governs an ADOPTED revision.
* **the reviewer's brief.** `GradeIntent` carries the same voice to
  `judge::intent_rubric`. In `Ceiling`/`Target` the retrieved look is stated as the
  BRIEF that a revision may not walk back (B2, unchanged and pinned byte-for-byte);
  in `Background` it is stated as CONTINUITY the judge must neither enforce nor
  penalise, and the refusal is re-aimed at the DIRECTION. This is not decoration:
  the judge BUYS revisions, and two of the three 2026-09-01 acceptance develops
  adopted a guided one, so an unconditioned rubric left the own-edit library as the
  stated brief for the reviewer that chose the recipe that shipped.

Retrieval is unchanged — `req.style > 0` still gates it, the direction still ranks
the exemplars and the look, IMAGE 2 still goes, and `STYLE_NEIGHBOURS` /
`STYLE_REF_IMAGE` / the look notes still disclose it. The skip has its own note,
`STYLE_BACKGROUND`, which names the adherence tier that caused it and the DIAL to
move — not a CLI flag: the note is persisted and re-rendered in three surfaces, two
of which have no command line.

**Why the look-library block and IMAGE 2 keep saying "match its grade".** The
reference block says the direction leads while `render_look_reference` still sends
"match its grade, not its content" and the IMAGE 2 sentences still say "Match that
LEVEL of grading". That is not a contradiction: the finished photo in those two
places is ranked WITH the direction among its terms — `look_ranked_by_direction` is
true exactly when the direction text carries a non-zero text or description weight,
and only then may the block claim "and direction" — so following that photo's grade
is following the direction rather than overruling it. The half that is NOT
direction-ranked — the shared tags of the photographer's own past RAW+XMP edits,
folded into `look_summary` — is the half the voice demotes, in the block and in the
judge's brief alike.

The adherence dial reaches every surface: `--adherence` (CLI), the Adherence slider
(desktop, and the browser page since v1.2.3), and the optional `adherence` field on
the analyze request body. That field exists because the dial stopped being prompt
intent only: without it every web develop carrying a direction was forced into
`Background` with no way back, while the other two surfaces could choose `Hint`.
Absent, it resolves through `DirectionAdherence::from_optional` to the same 0.65
every other surface defaults to, so an older client's request is unchanged.

With no direction, a blank one, or one at tier `Hint`, the block is byte-identical
to v1.2.2 at every Style value
(`style::the_no_direction_block_is_byte_identical`, whose three fixtures were
captured from the v1.2.2 build before the third voice existed).

### 4.7 Pixel retouch / heal (optional) — V2

A third, opt-in editing mode (`autoshade heal`, or the UI's **修图 · 去瑕疵** panel),
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
A full-resolution heal is the largest allocation this process makes, so it
takes the same one-at-a-time admission slot (`lib::full_res_slot`) the local
server's full-resolution requests take: the ticket is process-wide, not the
server's private static, because two full-frame buffers on one machine is the
shape that runs it out of memory no matter which surface asked for them.

### 4.8 Look matching / reverse-fit (`match`) — V2

The inverse of the advisor path: given the same frame twice — the untouched
source and a *target rendition* of it (a `reimagine` output, an exported JPEG,
any finished reference of that shot) — solve for the `EditRecipe` that
reproduces the target through our own engine ([`src/fit.rs`](../src/fit.rs)).
No target pixels are copied: the answer is global sliders + curves and,
optionally, semantic bitmap region adjustments or native luminance-range
adjustments. It applies at full sensor resolution; classic XMP carries the
representable global controls and native ranges, while semantic-region bitmaps
stay engine-only. Deterministic and key-free.

The method is deliberately **distribution-level, not per-pixel regression** — a
generative target is not pixel-aligned with its source, so only statistics are
trustworthy. A rank/gradient/pyramid structural-divergence reading selects the
global policy before any CDF solve. **Full** mode keeps four stages, in this
order: luminance-CDF tone matching (sampled
at the engine's own tone knots and least-squares solved against the engine's own
slider basis, with a ridge + penalised model-selection prior so numerically
equivalent but semantically ruinous slider combos lose); then saturation by
mean-chroma ratio, secant-refined through real renders and closed with a
do-no-harm check; then the per-band colour mixer — `hsl.saturation` and
`hsl.luminance` solved one ACR band at a time from that band's own population
statistics (weighted mean chroma, weighted mean Rec.601 luma, never paired
pixels), admitted by the same two-sided population gate the rest of the module
reads, with refusals typed and named and `hsl.hue` never solved; it is judged
twice, once where it is fitted and once after the cast curves against its own
absence on the finished frame; then per-channel CDF residuals as red/green/blue curves,
admitted only through four vetoes (aggregate error, foreign-hue, rotation
budget, hue fan) — each veto is a specific real-photo failure recorded at its const
block.

The fourth veto, the **hue-fan gate** (v1.2.3), closes the hole the first
three structurally cannot see. They all ask about a pixel's DESTINATION: how
far it travelled, whether it landed in a hue the target holds nowhere,
whether the aggregate improved. Three INDEPENDENT monotone channel maps do
something no hue-preserving control can and none of those questions reach:
they sort a single-hued region into several hues BY LUMINANCE. On the
Cornwall lighthouse-islet reverse fit (the pair v1.2.2 shipped as fitted —
closed by this gate in v1.2.3, made structural by the terminal re-read in
v1.2.4) the admitted curves moved the sky's mean hue 218.3° → 217.6°,
rotated no pixel past the 75° census at all (weighted re-hued share
0.000000), created 0.000000 foreign share and halved the look error
(0.0576 → 0.0334) — while splitting the sky's hue across luminance from a
1.6° spread to 33.1° in the delivered render: 226.8° in the dark half
(violet), 193.8° in the bright cloud (green-cyan). The gate re-reads the
rotation budget's exact census population — a measurable hue before (chroma
≥ 0.03), a visible tint after (chroma ≥ 0.04), evidence-weighted — bins it
by 15° hue class and by the evidence model's own luma bins, and measures the
widest circular gap between a class's slice mean hues MINUS the gap they
started from, so a class that was already fanned (content) or that the
curves rotate rigidly (a real global cast) reads zero. A class holding ≥ 5%
of the population that gains ≥ 15° of spread is refused. Calibration, all
measured on the analysis raster the fit itself uses: the wreck 37.6° and its
synthetic fixture 44.6° against the accepted haze-correction 7.8°,
canyon-warm 7.5°, canyon-gold 5.2°, hazy→vivid 2.7° — 15° is one full hue
class, 1.9× above the largest legitimate reading and 2.5× below the wreck.
It was verified end to end, not only on the census: at a 20° threshold the
Cornwall solve does not refuse outright (the mixer's do-no-harm loop halves
Aqua/Blue and refits until a milder cast measures 19°, which ships and still
leaves a 20.6° fan in the delivered sky); at 15° the refusal stands and the
delivered sky's spread is 1.6° — the same coherence the target's own sky
has — at a look error of 0.058 instead of 0.033. That 20° result is also
what the fix had to answer structurally rather than by calibration. The
do-no-harm loop re-fits after every shrink, so the solve can search for a cast
that clears the limit rather than give the cast up, and the 20° experiment is
exactly that behaviour caught in the act: a threshold nothing re-reads at the
end is a threshold the pipeline can walk around one admissible step at a time.

Since v1.2.4 the finished render is read once more, with the same census,
against the untouched base — the pair the user actually gets, rather than one
stage's candidate. Over the line, the cast curves are withdrawn and the frame
re-measured, and the recipe that clears is what ships. If the reading survives
the withdrawal the curves were not the cause, so they go back — a fit must not
pay look error for a fan it did not open — and the delivered reading is
disclosed with both numbers; that arm is not defensive, because tone and
saturation alone reach 12.9° of added fan on the `p36` calibration pair, which
carries no cast at all. Measured over the whole library battery (2026-09-02):
108 finished Full-mode renders, and the widest ADDED fan among them — the
reading this check judges, the same one the gate judges — is the coast
fixture's 14.2° against the 15° line, so the check fires on nothing in the
tree and changes no recipe. That is what a structural guarantee looks like
while the calibration above it is doing its job.

Two tolerances are stated here rather than discovered later. **The worst
case**: the gate judges the spread the curves ADD and subtracts the spread
the class arrived with, and that baseline is bounded by one class width —
which is 15°, the same number — so an ADMITTED cast can leave up to 30° of
ABSOLUTE in-class spread in the delivered frame. That bound is asserted on
the admitted haze pair, not promised. **The bin phase**: the classes are a
fixed 15° grid, so a coherent region straddling a class edge splits into two
and can fall under the 5% floor. That is the same grid-phase sensitivity the
foreign-hue veto has always had, and it is kept deliberately: reading the
census on one identical population is what stops the two gates drifting into
disagreeing about WHICH pixels they judge. Cornwall's convicted class holds
0.917 of the census population — the seascape's whole blue class, sky AND
sea, which the curves sort by luminance together; the row-defined sky alone
carries 0.561 of the hue weight. The 0.917 names a hue population, not a
region, and prose that calls it "the sky" is naming the wrong thing.

The refusal is disclosed with its readings, and a pair the pixel-aligned
gates already refuse keeps reporting exactly the note it reported before,
which is what holds those recipes byte-identical. The scope of that
byte-identity claim is exact, because the admission notes below APPEND to the
rationale and the rationale is part of the recipe: a pair whose cast is
refused (the viaduct) or never fitted is byte-identical to v1.2.2; a pair
whose cast is ADMITTED or PROJECTED is not.

### …and a projection, so a convicted cast is shrunk rather than thrown away

Refusing outright cost the showcase pair a third of its fit (look error
0.137 → 0.058 instead of 0.137 → 0.033, confidence 0.646 → 0.577), and the
fan gate's verdict is narrower than "this cast is wrong": it says "not in
this SHAPE". So when the fan gate is the ONLY gate that fails — the two
pixel-aligned vetoes, the unsupported-hue-range veto AND the aggregate ratio
gate all clear — the stage does not empty the curves. It walks them down a
one-parameter path and ships the best-PAYING point on that path that clears
(v1.2.3 shipped the strongest one; see the search's own section below).

The path gives up the CHROMATIC part first. Write `L` for the per-knot mean
of the three fitted outputs (the shape all three channels share — one curve
applied to every channel) and `dC = C − L` for each channel's deviation from
it. Then `C(t) = x + min(1, 2t)·(L − x) + max(0, 2t − 1)·dC`: `t = 1` is the
fitted cast, `t = 0.5` is one shared curve with no chromatic difference left
at all, `t = 0` is no curves. It is still three Lightroom RGB curves at every
`t`, so the recipe round-trips to XMP unchanged, and it is the same idiom one
stage up — shrink until the finished frame stops objecting.

The lower half of that path is there because measurement put it there. The
design this implements stopped at `L`, on the premise that one curve applied
to all three channels cannot fan a hue class. The premise is false, and the
showcase pair is where it fails: hue is a RATIO, so a shared curve moves it
wherever its slope changes, and Cornwall's shared shape has segment slopes
0.172 / 0.859 / 1.127 / 0.188 — its top segment nearly flat because the
fitted red curve clips at 179 from input 191 up. Measured on that shape
(2026-09-02): a dark sky pixel moves +0.2°, a mid one −3.2°, a bright one
−20.1° as its blue channel is crushed toward the other two, and the census
reads 17.3° of ADDED fan at `t = 0.5` — above FAN_DEG itself. A family whose
mildest member is still convicted can rescue nothing, so the path continues
to the identity, where the fan is zero by construction and the outcome is
exactly the old refusal.

The search reads the RENDERED candidate at every probe, never an
algebraic reading of the curves — the same rule every closed-loop stage here
follows — and re-judges each one by all four
gates from scratch, so a milder cast that makes the aggregate ratio fail, or
that trips a pixel veto the fitted cast happened to clear, is refused and
says so; and the strength budget's bound rides into that judgement exactly as
it does for a fitted cast. It runs in three phases: a 12-step bisection for
the admissible frontier `t_max`, a fixed eight-cell sweep of `(0, t_max]` that
keeps the best-PAYING admissible probe, and eight golden-section iterations on
the winning cell so the answer is not quantised to the grid.

Two thresholds are the projection's own. It must
clear **half** the refusal line — `FAN_PROJECT_DEG` = 7.5°, not 15° —
because 15° is where the calibration put the visibility edge (the FAN_DEG=20
experiment shipped a 19° cast that left 20.6° of delivered fan) while the
widest fan the gate passes on its own merits is the haze correction's 7.8°: a
cast the fit CHOOSES to keep must be no worse than one it already accepts.
And it must buy more than `FIT_QUANT` of absolute look error — the fit's own
quantisation budget, the same constant the terminal do-no-harm check uses —
because the gates decide whether a MEASURED cast may ship, not whether an
INVENTED milder one is worth shipping, and marginal gain does not earn
regional risk.

Those two thresholds are applied in different PLACES, and that is a soundness
requirement rather than tidiness. A bisection can only find the largest member
of a downward-closed set, and only the gates-and-fan half is downward-closed:
the fan grows with `t`, and every gate clears as the curves approach the
identity. The gain runs the other way — it falls to zero at `t = 0`, where
there are no curves at all — and it is not monotone in between: measured on
the coast fixture's candidate (2026-09-02) it reads 0.00104 at `t` 0.25,
0.00190 at 0.35, 0.00169 at 0.40, 0.00187 at 0.50, a wiggle the size of
`FIT_QUANT` itself. Testing both inside the loop makes the clearing
set an interval `[a, b]` with `a > 0`, so a probe that fails on the GAIN
pushes the search away from the band and out at `None`, refusing a pair whose
refusal sentence then claims the whole path was searched. So the bisection is
pointed at admissibility alone, and the gain question is answered by SWEEPING
the admissible interval the bisection found. The bar is then applied once, to
the MAXIMUM over that interval, which is what makes a refusal honest: `None`
means "nothing on this path pays" rather than "the strongest point on this
path did not pay".

v1.2.3 judged the frontier alone and wrote the cost of doing so down: a shrink
that pays only at a milder `t` was not found, and the two-family HSL pair was
refused although every `t ≤ 0.25` on its path was admissible and paid
0.0019–0.0033 while the frontier read −0.012. v1.2.4 closes that, and the
sweep is what a bisection could not do soundly. The pair now ships a shrink at
`t = 0.318` whose look error is 0.885 of the error without it, and its finished
residual falls 0.0256 → 0.0227 — under the `FIT_QUANT_CLEAN` = 0.025 floor at
which the unrepresented-controls disclosure returns early because there is
nothing left to explain, so that pair ships a better fit and a shorter
rationale, and the `hsl` sentence it used to carry is asserted on a wider band
gap instead. Nothing else in the tree moves: the calibration pairs p36–p41 and
the generated pair are byte-identical before and after, because on none of them
does the fan gate convict alone.

The precedence is the design's, exactly: the fan gate must be the ONLY gate
that convicted. A pixel-aligned veto says the DESTINATION is wrong and no
point on the path makes a wrong destination right; the ratio gate says the
curves did not buy enough, and a weaker version of curves that did not pay is
not an answer to that — it would also ship a sentence naming the fan and only
the fan, disclosing one of the two verdicts its curves had to survive. Either
way the pair stays refused, with the note it already had.

A projected cast discloses at least what an ADMITTED one does, and for a
sharper reason: these are curves the fit INVENTED to answer a conviction
rather than curves it measured off the pair, so the two pixel-aligned readings
matter more here, not less. Its head note carries the conviction, the shrink,
the ratio against its bound and the re-hued share, and the admission's own
foreign-hue clause follows it — the same key, so there is one sentence and one
translation, and the same measured / not-measurable pair so a census that
never ran is never published as `0.000`. A projected curve that lands on the
identity at every knot is emitted EMPTY, per channel, exactly as the fit
leaves a channel it never fitted, so `t = 1` reproduces the fitted curves byte
for byte including the empty ones and no recipe ships five knots that do
nothing.

Ordering is the whole of the byte-identity argument. A pair the pixel vetoes
refuse is refused unprojected, so the viaduct's `match` recipe is byte-for-byte
what it was. The rescue runs on exactly two calls, and both of them produce
the recipe the user gets: the `fit_cast_stage` after the mixer's do-no-harm
block, and the 4b do-no-harm loop's re-fit, which REPLACES that recipe one
saturation step down. The mixer's do-no-harm loop judges both of
its branches with the cast the gates MEASURED, because its question is about
the MIXER and an invented compromise must not out-vote a per-band solve the
evidence supports — with the rescue live in every call, four fixture verdicts
moved that have nothing to do with this feature (canyon-warm's mixer flipped
from withdrawn to attached, the two-family HSL pair's the other way; that
experiment's four are a superset of the THREE that survive the confinement,
named below).

The 4b call is exercised and its SUCCESS is UNREACHABLE, which is a
measurement rather than an impression. Instrumenting the loop body and running
the whole library battery with all seven calibration pairs present
(2026-09-02) logs 186 entries — 107 on the error arm, 140 on the hue-guard
arm, 62 on both — of which 9 re-fit a fan-convicted cast, so the rescue is
entered there with something to answer. Every one of those 9 is ALSO
rotation-blocked, so `earns_projection` answers `None` and the search is not
even called: 0 of the 186 earn a projection and 0 come back projected. At the
stage's own call site the two gates do come apart (67 fan-only refusals in 545
stage runs), so what couples them is the stepped-down state rather than the
gates themselves; about forty pairs built to separate them reproduced the
coupling every time. The projection's arithmetic is pinned directly at
`search_cast_projection`; what no test witnesses is the ROUTE through this
loop, and the census is why that is a dead end rather than a gap. The comment
that used to stand at that loop, "no current fixture reaches the loop body",
was false and is corrected in place.

Cornwall, global stage, `match` without `--zoned` (2026-09-02): the fitted
curves would have opened 37.6° in a class holding 0.917 of the frame's
measurable colour; shrunk to `t = 0.363` they open +7.4°, and the fit reports
look error 0.137 → 0.030 at confidence 0.664 — better than the v1.2.2 render
that carried the defect (0.033 / 0.646). The delivered sky's hue spread
across luma octiles is 10.5°, against 33.1° in v1.2.2, 1.6° when the cast is
refused outright and 1.6° in the target itself; its mean hue is 216.7°
against the target's 215.3°. The 15° target was measured beside it and
rejected: it recovers a little more (0.026 at confidence 0.680, `t = 0.483`)
by delivering a 15.3° sky fan, and the recovered 0.004 of look error is not
worth a fan the user cannot undo.

THREE fixture verdicts moved, and they are named here because a feature that
moves a calibration owes the reader the list.

**One.** A pair whose cast is convicted can now land where the terminal
do-no-harm check used to reset the whole recipe to the calibration base: the
canyon-warm regression went from 0.0387 → 0.0387 at the 0.25 confidence floor
to 0.0387 → 0.0339 at 0.406, with its delivered sky at 216.9° against 213.9°
before and no fan at all (that fixture's sky is one flat colour). Its joint
reading moves with it, 0.1796 → 0.0437, which takes it out of the joint
family's refusal band and leaves canyon-gold as that band's only member — the
whole record of six fixture readings was re-measured on this tree and had
drifted in every row.

**Two.** Canyon-warm's protection MIGRATED. Because its mixer now attaches
first, the cast curves re-derived against that state rotate nothing at all
(0.0000 of the frame, against 0.1250 with the mixer neutralised), so the
rotation veto no longer fires on it and what stops the violet is the
composition of the mixer and the projection. The `ROT_DEG`/`ROT_SHARE`
calibration test therefore reads the gate on the PAIR — mixer neutralised —
which is what that calibration is about; the delivered-frame guard is where
the protection is measured, and it reads 216.9° against a ±30° band around
213°.

**Three.** The two-family HSL fixture ends where it began. With the gain bar
tested inside the bisection it shipped a rescue worth 0.0006 of look error,
which took its finished residual under the `FIT_QUANT_CLEAN` = 0.025 floor and
cost the pair the sentence naming `hsl`. With the bar applied once, to the
winner, that rescue is refused, the residual is back at 0.0256 and the
disclosure is back with it — the fix-up's soundness change paid for itself
here.

What the projection does NOT do is pay the gate's deliberate cost: a scene
genuinely lit at two colour temperatures needs the fan, so every point on the
path reproduces proportionally less of what it needed, no `t` both clears and
buys anything, and the refusal stands — now saying that the shrink was tried.
That case has a fixture of its own since v1.2.3.

The colour stage's ADMISSION is disclosed too, from the same release. Every
way of producing nothing already had a note, and the strength budget
disclosed when IT bought a marginal cast, but the commonest outcome of the
whole stage — the curves shipped on their own merits — reached the user as
an unexplained presence. An admitted cast now carries the four gates' own
readings so the admission can be checked rather than believed — across THREE
notes, not one, because two of the four can ABSTAIN. The look-error ratio,
the bound it was judged against and the re-hued share are always measured and
ride the head note; the foreign-hue share and the hue fan each get a measured
clause and a not-measurable clause, so a census that never ran (a target with
no chromatic mass; no hue class region-sized across two luma slices) says so
in words instead of publishing `0.000` as though it had been measured. The
ratio is stated against its bound rather than as "cut the look error to": the
ratio arm rejects only when the evidence is ALSO unidentifiable
(`identifiability < 0.25`), so an admitted ratio may legitimately exceed 1.0,
and the old wording then stated the opposite of its own measurement. The
bound is `budget.cast_ratio` — the value the path actually used, which the
strength budget widens from 2.0 at the shipped default up to 3.0 — not a
fixed constant. The fan is SIGNED, because three channel curves can narrow a
class's spread as easily as widen it, and it is printed to one decimal,
because at whole degrees the admitted haze pair's 14.6° rendered as "+15
degrees, against a limit of 15" — a sentence stating a violation of the
number beside it, over a reading that had passed.

**Atmosphere** mode instead uses bounded robust exposure, white balance,
a five-point tone curve, saturation and the same evidence-gated per-band
colour mixer, never per-channel curves, and caps the
reported confidence because develop controls cannot reconstruct the changed
structure. Atmosphere mode is entered because structure diverges; its
instruments are the budgets (EV ±1, WB gain [0.80, 1.25], saturation ±30,
curve slope [0.5, 1.5]) and the population facts, not structural survival.
The budget is derived from the requested strength: 0.0 uses EV +/-0.5,
saturation +/-15, per-band +/-6, WB [0.90, 1.12], ratio 1.20, WB rotation share 0.05, Full
cast-error ratio 1.5 and slope [0.7, 1.3]; 0.65 keeps the calibrated EV +/-1,
WB [0.80, 1.25], saturation +/-30, per-band +/-18, WB rotation share 0.05 and
slope [0.5, 1.5], with the historical Full cast-error ratio 2.0; 1.0 permits EV +/-2.5,
saturation +/-60, per-band +/-45, WB [0.50, 2.00], ratio 3.0, WB rotation
share 1.0, Full cast-error ratio 3.0 and slope [0.25, 3.0]. Between 0.65 and 1.0 the WB
rotation share opens linearly (about 0.593 at 0.85). The WB gain ratio and the
Full look-error admission ratio are independent budget dimensions. At or below
default, an out-of-budget WB remains as-shot exactly as in the pre-F1 path.
Above default, an out-of-budget WB is scalar-shrunk from as-shot along the
fitted move (log-space Kelvin and linear tint), then the persisted rounded WB
is checked again. Its pre/post renders also pass the foreign-hue veto and the
weighted rotation allowance; a failure restores as-shot and is disclosed with
a typed note.
The confidence cap is the measured look-error ladder capped by the strength
budget: it is 0.50 at strengths 0.00/0.65, 0.414 at 0.85, and 0.35 at full
strength. Unsupported movement is disclosed from strength 0.85, while coherent global casts remain
measurable and foreign-hue painting remains vetoed.
Its report therefore has one ruler: frame error, harm, confidence and disclosure
all read a structure-blind re-aggregation that preserves one-sided, sparse and
minimum-share population vetoes. The structural model is carried separately
only for Full zones and the detail stage. Every Atmosphere report discloses the
structurally withheld ranges and explains that they do not constrain its bounded
atmosphere controls. Since R30 batch 1 it also states the POPULATION those
controls were read over, and since R30 R2 that population is no longer always
the whole frame. With NO usable cross-image correspondence field the population is
unrestricted and the report says so: both controls are read WHOLE-FRAME, and
how much of that population has no counterpart reads as NOT MEASURED rather
than as zero. The two controls read it differently, and since step 9 they read
it with different statistics. EXPOSURE is a ratio of two whole-frame weighted
luma medians, which pairs the two frames as DISTRIBUTIONS and so presumes both
describe the same content — the presumption selecting Atmosphere denies.
WHITE BALANCE no longer does: it is a weighted median of the PER-PIXEL log
ratio, one cloud of per-pixel colour changes rather than a ratio of two
marginals, because three independent per-channel medians on a bimodal frame
draw their halfway points from different sub-populations and their ratio is no
pixel's colour. On the crate's own `flat_sky_to_cloud_deck` fixture, where the
land half is byte-identical between the two frames and the sky keeps its exact
chromaticity vector with only its luminance redrawn — so no pixel changed
colour at all — the marginal form read K 4400 / tint +55.2 and the per-pixel
form reads the anchor back. WITH a field, the
SHARED-CONTENT population replaces the whole frame on BOTH sides: target
pixels no confident source cell maps onto (generated content that is not a
rendition of this frame) and source pixels whose content the target replaced
(evidence with nothing left to compare against) are dropped before the two
medians are read, and the report states the evidence mass each side kept.
Both sides, because `median(target)/median(source)` is a ratio of two
populations and moving one of them onto the shared content while the other
stays whole exchanges the mismatched pairing for a louder one — measured on a
synthetic pair whose invented region is the brighter 60% of the frame and
therefore owns every whole-frame median, where the truth is `gr/gb` 1.2181 at
0.00 EV: whole-frame answers 0.911 at +0.694, TARGET-side only answers 1.945
at −2.867, SOURCE-side only answers 0.512 at +3.593, and the shipped
two-sided cut answers 1.216 at +0.032.
The cut is BINARY at the same 0.5 the disclosure publishes, because the
sidecar's confidence measures trust rather than mass and a confidence-weighted
average would let a large barely-trusted population outvote a small certain
one. The mask is the very bitmap the unpaired share is counted from, projected
onto the analysis raster by the same nearest-cell rule, so the sentence and
the population can never disagree. When either side retains less than
`SHARED_POPULATION_MIN_RETENTION` of its own evidence mass — the evidence
model's own range-survival floor, 1 − `DIVERGENCE_ZONE` = 0.35 — the
restriction is REFUSED, the whole-frame medians stand, and a second sentence
says why: solving a global control on a corner of the frame is the same
failure in a different costume. Measured on the seven-pair corpus: three pairs
never reach the solve at all (Full mode consults no field), the island pair
retains 84%/76% and restricts, the calibration pair 58%/50% and restricts, and
`p37` retains 11%/7% and refuses.
The restriction moves the reference population of those two controls and
nothing else — the structure-blind ruler, mode selection, the 0.50 confidence
cap, the tone curve, the saturation chase and the per-band mixer are untouched,
and a pair with no field is byte-identical by construction rather than by
arithmetic.
It is also a PARTIAL repair on real pairs, and the size of what it leaves is
measured rather than assumed. On the island pair it moves the white balance
4400 K/−20.6 to 4350 K/−33.1 and the bottom third's closures by less than
three points in mixed directions: linear R/B −72.93% → −75.64%, chroma
+40.28% → +40.59%, linear luma +48.08% → +48.64%. The reason is that DIFT's
confidence does not isolate that pair's replaced sky — the cells it drops are
61–69% sky against a 46–47% frame-wide base rate — and even an ORACLE
population with the sky removed by segmentation, which no shipped instrument
can identify, only reaches −35.05%. No choice of reference population in this
estimator closes that gap, because the residue is not a population defect: the
pair's whole-frame per-channel MEAN R/B ratio is 0.9991 — the target's white
balance IS the source's — while the per-channel MEDIAN ratio is 0.8293,
because on a bimodal frame the three channels' independent medians are drawn
from different sub-populations and their ratio is no pixel's colour. That is a
defect in the robust STATISTIC rather than in its population; R2 registered it
and STEP 9 discharged it. The estimator is now a weighted median of the
per-pixel log ratio over ONE population, and when the correspondence field is
readable it reads the target through that field's own remap rather than by
raw index — the same field that decided WHICH pixels are shared also decides
WHICH target pixel each source pixel is paired with, because taking the
population without the pairing is what lets a RECOMPOSED pair (the same
content, moved in frame) be read as a colour cast. What step 9 does NOT claim
is that this closes the island's bottom third: the target's white-balance
demand there is spatially OPPOSITE to its demand over the top two thirds, so
no single global gain can serve both halves, and the honest contribution is
removing a wrong-way error rather than supplying a right-way one.

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
piecewise-linear chords sag ~10/255. `--zoned`
([`src/fit_zoned.rs`](../src/fit_zoned.rs)) is one automatic local-fit entry:
it fits globally first, then uses mutually exclusive producers. Successful
segmentation adds up to four disjoint semantic class bitmap corrections (one
OneFormer inference per frame); each accepted region selects Full or Atmosphere
independently and confidence is the minimum of global confidence and the worst
accepted-region confidence. The historical two-region route is the default;
four regions are opt-in through `--regions 4` or the GUI control. A disabled or
unavailable sidecar instead runs the pure-Rust luminance-range pass
([`src/fit_zoned/range.rs`](../src/fit_zoned/range.rs)); if neither
producer keeps a correction, the global fit ships unchanged. Segmentation
success does not derive range bands. The same structural statistic independently selects **Full** or bounded
**Atmosphere** policy for each zone; structural divergence never drops a zone
by itself. Every candidate must preserve mask-weighted texture energy and
clipped-luma share; the same analysis render also feeds a boundary-continuity
gate that reads the luma bow only in the mask's 5%-95% transition band, then
applies the largest shared, direction-preserving differential shrink `k`
inside its calibrated budget. Since step 9 that bow is the bow the correction
INTRODUCED: the same band on the render WITHOUT the correction is subtracted
first, and it is TRANSPORTED through the settled sky's own linear multiplier
before it is — `encode(M · decode(L))` with `M` the settled sky's linear
ratio — because every zone dial is multiplicative in linear light, so a band
pixel carrying a content rim above the settled sky moves MORE in absolute luma
under the same multiplier and a plain difference of differences would charge
that to the correction. On a hazy 12x4 probe under ONE multiply applied to the
whole frame, where no seam exists by construction, the plain difference reads
+0.0201 at a +1 EV dial (1.68x the budget) and the transported difference
reads exactly 0.0000. The reading is ranked by MAGNITUDE at the 90th
percentile, exactly as the cross-boundary step already ranks its own samples
and for the same reason: a correction that darkens its side of a border is as
visible a seam as one that brightens it. Before step 9 the rim was ABSOLUTE —
measured against the settled sky alone with no reference at all — so a bow the
scene already carried under the feather was charged to whatever correction
happened to be under test; on the island pair that dropped BOTH zones for a
+0.060 reading of which `k=0`, no local correction, still left +0.058.
Consequently the drop branch (`k=0` still over budget) is now an INVARIANT
rather than a policy branch — `k=0` zeroes every additive dial and drops every
gain, so the `k=0` render must reproduce the reference exactly — and it is
kept, and asserted, because a deleted branch cannot catch the engine bug it
now names. That ruler needs a transition band to read, so it serves the
FEATHERED segmentation masks only. Every hard-edged raster — spatial tiles
and free masks, written 0/255 — is measured instead by the CROSS-BOUNDARY STEP:
paired samples 1.5 px either side of the mask's 50% contour, `(inside minus
outside)` on the corrected render minus the same difference on the render
WITHOUT the correction, ranked by magnitude at the same 90th percentile.
Differencing against the uncorrected render is what makes a subject edge lying
under the mask border cancel on BOTH rulers, so only the discontinuity the
correction itself introduced is budgeted. The two constants are SEPARATE
(`ZONE_BOUNDARY_RIM_MAX`, `ZONE_BOUNDARY_STEP_MAX`), both `0.012` today:
they were one constant until step 9, which meant re-deriving either ruler's
budget silently re-tuned the other, and the island's three accepted tiles
sit 1.7%-3.3% below the step ceiling where that would have moved them.
Since v1.2.2 the step constant is a CEILING rather than a flat budget:
0.012 was calibrated where neighbourhood contrast masks a discontinuity of
that size, and a measured tile seam in clean sky sat exactly on it at 7.8
sigma over a mask-free neutral control. Each crossing is therefore charged
against its own per-crossing budget — the larger of the scene's own step
across the same feet and `BOUNDARY_STEP_SHAPE = 3` times the correction's
own same-side slope read off the frozen k=1 candidate (the minimum over
two consecutive baselines, so the resample-and-refine collar — the seam's
own soft shoulder — earns nothing while a persisting ramp keeps full
credit), clamped to
`[BOUNDARY_STEP_FLOOR = 1/255, ceiling]` — and the gate compares the
charged 90th percentile alongside the still-disclosed raw step. A crossing
whose context reaches the ceiling is charged its raw step bit-for-bit, so
textured borders and true ramps are governed by exactly the number they
were governed by before; a crossing in smooth sky must fit inside what its
own neighbourhood can actually mask. The soft rim and luminance-range
families keep the scalar: the rim ruler samples only inside a feather,
where the correction is a ramp by construction, and the range ruler admits
only locally smooth crossings by rule.

**The range family's half of that sentence is measured (2026-09-01), and it
holds for a reason the step ruler does not share.** `range_transition_rim`
never differences the scene away: it admits a neighbouring pair only where
the REFERENCE crossing is already smooth (`|Δl| ≤ 2.5/255`) and then reports
the RENDERED gradient there, so the scene's own gradient sits inside the
reading and is spent before the correction adds anything. That argument was
already in the tree — the comment inside `range_transition_rim` has said
since v1.2.2 that a graded context here is capped near 2.5/255 by
construction and therefore has no dynamic range; what 2026-09-01 adds is the
measurement behind it and a test. Charging this ruler the way the step ruler
is charged would OVER-tighten it, not loosen it: the v1.2.2 charge is
`raw × (MAX ÷ budget)` with `budget` clamped at or below `MAX`, so the
multiplier is ≥ 1 by construction and can never weaken a gate; and here the
context term is the admitted crossing itself, capped at 2.5/255 = 0.0098 and
so always under the 0.012 ceiling, which would collapse the pass condition to
"scene + correction ≤ scene" — no steepening admitted anywhere.

What the budget buys is a **p90**, not a per-crossing cap. The reading is the
90th percentile of |signed bow|, so a tenth of the crossings rank above it
and are not bounded at all. At that rank the widest crossing the window
admitted on either real pair measures 0.00978 luma (2.49 codes) against the
budget's 3.06, so where the scene already sits at the top of the window a
correction adds 0.57 of a code and no more, and where it is flat the ceiling
is the whole 3.06 — 1.22× the steepest crossing the window is willing to call
smooth. The measured maxima sit above both, as a p90 gate permits:
calibration 0.00978 → 0.01217 (1.24×), viaduct 0.00978 → 0.01407 (1.44×, i.e.
1.09 codes added at that one crossing, past the 0.012 budget itself).

Driven first-party with segmentation and correspondence made unavailable so
`match --zoned` takes the fallback, a band attaches on the calibration pair
(luma [0.471, 0.765], −0.56 EV) and on the stone viaduct (luma [0.118,
0.882], −0.80 EV and saturation +23); the Cornwall pair attaches none. TWO
RASTERS are in play and the table says which. The ENGINE gates on the 384-px
thumbnail it develops itself, and disclosed `k=1.000` over 18 651 and 1 214
crossings. The readings below are a transcription of the same statistic over
a full-resolution develop downsampled to a 384-px long edge, on the basis the
engine passes it — `&global_px`, the globals-only twin with no range masks.

| transcription, twin basis | calibration | stone viaduct |
|---|---:|---:|
| crossings | 18 297 | 1 224 |
| uncorrected p90 (max) | 0.00392 (0.00978) | 0.00874 (0.00978) |
| delivered p90 (max) | 0.00230 (0.01217) | 0.00857 (0.01407) |

The correction moved the RANKED reading down on both pairs. The delivered
transitions are ramps rather than steps: over the populated 1-code bins of
the delivered transfer (twin luma → delivered luma at 1200 px) the
calibration band shows 0 reversals in 30 bins with a minimum slope of +0.019
and a maximum of +0.964 — compressive, 30 input codes into 9.9 delivered ones
with 9 of the 30 bins under slope 0.1 — and the viaduct 0 in 19 with +0.855.
The same estimator over the zero-weight control region, where the true slope
is exactly 1.000, reads +0.942 to +1.039: a ±0.05 noise floor, which puts
+0.019 one noise unit away from a reversal and is why the magnitude-blindness
below is registered rather than dismissed. `scripts/rim_overshoot.py` reads
mean 0.0006 / p90 0.0018 / max 0.0082 luma at the calibration transition,
against its own control of exactly 0.0000.

The seam-style statistic that decided v1.2.2 does not decide this one, and
its own numbers say why. Stepping across the calibration band's contours in
8-bit codes at 1200 px, all four measured rows: lo_outer +9.05 codes at
z +10.08 on the twin basis but +2.83 at z +1.94 on the neutral one; lo +7.18
at z +6.99 twin and +8.28 at z +8.92 neutral — with p90 |·| 22.84 codes, 26.8
times the control's sd of 0.853. A fivefold swing at ONE contour between two
honest bases is itself the evidence that the reading is basis-dependent. A
tile edge is an arbitrary rectangle laid across continuous sky, so any step
there is an artefact; a range edge IS an iso-luminance contour of the
photograph, so a difference across it is the correction doing its job — and
the transfer above shows the ramp compressing (maximum +0.964 inside it), so
those 9 codes are the scene's own gradient being flattened rather than an
induced step. Beside the v1.2.2 seam table (neutral +0.15 codes at 1.3 σ, the
v1.2.1 tile +1.59 at 7.8 σ, the fix +0.92) the range family is a different
SHAPE of thing, not a smaller number of the same shape.

Two limits go on the record with it. The mask-free spatial ruler is not
applicable at the viaduct's contour, by its own numbers — its 60 px plateau
windows must bracket the transition, and there the band's own spread (40.4
codes) exceeds the plateau gap (22.1) on 201 of 231 columns. And this ruler
ranks MAGNITUDE, so it cannot tell a preserved gradient from an inverted one
of the same size: on a synthetic 16-bit grey ramp a 1.5/17 band reverses from
−0.56 EV (a 2-code dip, 26.5 codes at −1.50) and a 1/17 band from −0.35 EV,
while `rim_overshoot.py` reads max 0.0000 over its full n = 1024 on every one
of those MEASURED rows, because the difference it ranks stays monotone. The
widest ramp the producer emits (2/17) was unmeasurable by that instrument on
that probe rather than measured clean — it needs 180 px of margin each side
and the probe's locator landed exactly on row 180 of a 512-row frame, so every
column was rejected and n = 0.

v1.2.4 re-cut the probe and closed both gaps. The frame is 64 x 1020 with a
4/17 band, the mask-free instrument's own 60/60/60 geometry is ported into the
test, and the band's luminance POSITION — the third axis the old probe pinned
— is swept over five values, so every cell returns n = 64 of 64, the 2/17 rows
included. Over 3 ramps x 5 EVs x 5 positions the mask-free ruler reads 0.0000
in all 90 cells while the delivered tone order inverts by up to 38 codes; the
widest ramp is the safest of the three at every cell and still inverts from
−0.80 EV upward, and position alone moves one row from 0 to 6 codes. The
answer ships with the measurement instead of after it: `range_transfer_reversal`
reads the depth of the delivered transfer's non-monotone excursion and
`enforce_range_boundary_gate` applies it beside the rim, shrinking a band that
inverts the tone order to the largest amount that does not
(`RANGE_TRANSFER_REVERSAL_MAX` = 0.5/255; the control reads exactly 0.000000 in
all 15 of its cells and both real pairs read 0). The rim ceiling itself is
unchanged and was never what was wrong.

For the step family a reading of ZERO measured crossings is a refusal, never
a pass — until 2026-08-30 the rim ruler returned `0.000` from an empty
transition band for every hard raster ever gated, and the gate read that as
comfortably inside budget. A Full-zone correction then uses the three-arm gate
(v0.26.1, third arm R30 batch 1): halve the zone error, land it at/below an
absolute matched floor (0.02 of linear-mean error, brightness within a quarter
stop — the floor lives in scale-dependent linear light, so the EV companion
rides both absolute yardsticks) with a real ≥20% gain, or be STRICTLY BETTER —
an absolute zone gain over `ZONE_MIN_ABS_GAIN = 0.012` while the frame-global
reading does not regress at all. The third arm exists because the first two are
ratio yardsticks with nothing absolute in them: the calibration land zone
improved 0.078 → 0.054 with the frame moving −0.00004 and every quality gate
clear, and was dropped for landing at 69% of its start rather than 50%. It pays
for that relaxation on the frame side, where zero regression is stricter than
the semantic route's own `0.02` drift insurance and equal to what the spatial,
range and free-mask routes already demand; a correction admitted by it carries
its own typed disclosure naming both readings and naming `ZONE_TEXTURE_MIN` as
the one safety gate known NOT to discriminate. An Atmosphere zone uses
zone-local do-no-harm instead, and the absolute arm never reaches it. A zone already inside the
observed matched DOMAIN (≤0.012, same EV companion) is left alone with an
honest "already matches" note instead of being dialled, regressed, and
reported as a dropped improvement; zones between the two yardsticks are
always attempted — the skip line and the acceptance floor are split
constants precisely so nothing fixable is declined untried. The GUI's **反推 / Reverse-fit** action drives the
same two entry points (`fit_recipe`, `fit_recipe_zoned`) and lands the
result as an editable variant.

The fallback range producer reuses the global fit's 17 rank-paired luminance
evidence bins. After the global render it joins contiguous signed residuals at
`0.03`, keeps at most four evidence-supported runs, and discloses every merge
or per-band abstention. Each accepted band is fitted once, in ascending-luma
order, through the same robust weighted estimator, correspondence composition,
share/mismatch/already-matched/local-quality gates, and parameterized frame
gate as a semantic zone. Semantic zones retain their measured `0.02` drift
insurance; a range band has zero drift tolerance and survives only when the
composed evidence-weighted frame is neutral or better. Source weights are
re-derived from the current rendered stack before each fit; overlapping
estimator ramps are normalized to sum to at most one, while the correction's
movement coverage remains its own raw, pre-normalization range ramp. Ramps span
one to two bin widths, and one final value-transition gate
applies a shared direction-preserving bisection shrink against the range
path's own `0.012` rim budget (`RANGE_BOUNDARY_RIM_MAX`, which has always been
its own constant). A native correction uses `MaskRole::Custom`, a deterministic English
name, and the full-frame sentinel `Linear { zero_x: 0.5, zero_y: -0.8,
full_x: 0.5, full_y: -0.4 }` intersected with `RangeMask::Luminance`; the XMP
reader and writer therefore need no new grammar. Color-range partitioning is
outside this step.

**Evidence verdicts follow the population a correction moves (B1,
2026-08-27).** `EvidenceModel::scoped(tp, source_zone, target_zone)`
([`src/fit.rs`](../src/fit.rs)) re-aggregates the same 17 luma bins and 8 hue
bands over one zone's soft memberships: target luma bins are rank-paired
within the zone's own target members at the source:target mass ratio, and the
structural-survival gate (`1 - DIVERGENCE_ZONE = 0.35`) and the per-pixel
spatial confidence are unchanged, so over the whole frame the scoped view is
the model itself byte for byte (pinned by test). `EvidenceModel::structure_blind`
re-aggregates the frame with structural survival and per-pixel withholding off
but population facts intact. An Atmosphere report scopes that blind model for
Atmosphere zones; a Full zone scopes the separately retained structural model,
and the frame-law judge remains on the report's single blind ruler. Detail is
the other structural consumer because texture identifiability is a structural
fact. Every evidence statistic
pairs source pixel `i` with target pixel `i`, so the two analysis rasters
share ONE geometry by construction: `fit::analysis_pair` thumbnails the
source and thumbnails the target into exactly that width and height with
the same box operator (`thumbnail_exact`; a Lanczos3 target against a
box-filtered source was measured to move a same-scene fit from 0.019 to
0.034 by kernel asymmetry alone); an equal-shape pair is therefore
byte-for-byte the two thumbnails it always was, by construction, and
every producer (global fit, rescore, semantic zones, luminance ranges, tiles)
reads the pair through that one helper. Before this, the two images were
thumbnailed independently and a ONE-ROW rounding difference (1600x1067 ->
384x256 against 1600x1069 -> 384x257) made `structure_divergence` return
`matched` on the unequal lengths, the same-content verdict came out true,
and no evidence range could be withheld -- the structural gate was silently
off for every such pair, the calibration pair included. The aligned-prefix
arithmetic inside the model (`0..min(source.len(), target.len())`,
population and both movement audits over the same mass) is the defensive
form of that contract. Semantic zones and quadtree
tiles ask their tone/colour vetoes of the view scoped over the coverage their
raster moves (`ZoneAttachment.coverage`; a tile's estimator weights are
evidence-weighted and would hide the withheld pixels its raster still moves),
tiles derive their per-pixel weights from their own view, and the blind-move
audit's 5% region line is a share of that population (`EvidenceModel::
population`) rather than of the frame -- a depth-2 tile is 6% of the frame, so
under the frame line its blind half could never be a "region". Range discovery
and composed-frame arbitration use the frame model, while each range
attachment's movement vetoes use the raw ramp of its own `RangeMask`. On the calibration
pair this ends the collateral veto in which the replaced sky, sharing the
land's luma bins, withheld the land's tone controls; with colour withheld the
skip line is now asked of tone alone, so the land (luma residual 0.004, under
the 0.012 skip line) is declared already matched instead of being dialled
+0.10 EV for a hairline gain that regressed the frame 0.0179 -> 0.0193. Its
remaining gap is chroma withheld by the hue doctrine (Blue one-sided). Live
A/B against the step-9 executable (user-ratified 2026-08-27): on the GUI path
(neutral development) the old code attached a land +0.08 EV that worsened the
land's own residual 0.041 -> 0.045 and a tile r2c0 (-0.24 EV, gains 1.30/0.86/
0.79) that undid most of that; B1 keeps the sky only, frame 0.0175 -> 0.0180.
On the RAW CLI path the range band [0.118, 0.294] stays tone-withheld -- a
value range spans replaced sky and land alike, so its own population is blind
(the grid's win was spatial x value, B2/B3 territory) -- and r2c0's warm gains
are withheld because Blue/Purple are one-sided inside the tile (the old frame
share let them through): 0.0549 -> 0.0452 -> 0.0369 against 0.0345. A zone
whose dials come out neutral is still dropped without a note (registered
follow-up).

Shared analysis geometry and the structure-blind Atmosphere ruler (2026-08-27,
user-ruled). Every GUI-path figure in the two paragraphs above (0.0175 ->
0.0180, the +0.10 EV land dial, the 0.0179 -> 0.0193 regression) was measured
with the structural evidence gate silently OFF: the 1600x1067 neutral
development thumbnails to 384x256 and the 1600x1069 target to 384x257, and
`structure_divergence` answered `matched` on unequal lengths. The RAW-path
figures (0.0549 -> 0.0452 -> 0.0369) had the gate on, which is the whole "3x
gap" between the two paths. With `fit::analysis_pair` both rasters share one
geometry and the gate is live everywhere. The calibration pair then reads
D = 0.49 (Atmosphere); its dark-land ranges survive structurally (0.54-0.57 on
luma [0.12-0.29], 41% of the frame) while the mid-tones the generator
re-rendered (0.10-0.33 on [0.29-0.59]) and the replaced sky (0.08-0.18 on
[0.59-0.82]) do not, and under the old range vetoes every global atmosphere
move was reset (0.057 -> 0.057; tiles alone reached 0.0362). Under the ruled
doctrine the report reads its population ruler: neutral -> target gives EV
-1.00, WB 7100 K / tint +22, saturation 0 (Aqua/Blue one-sided), a five-point
curve, clarity/texture 0, ruler 0.189 -> 0.096, confidence 0.25; with
segmentation off one tile attaches (-0.56 EV), with it on the sky zone
(-0.08 EV); the RAW path gives EV -1.00 and a sky zone at -0.27 EV (0.194 ->
0.108). On a user-visible pixel ruler (`scripts/pixel_ruler.py`: mean CIE76
dE of the render against the target at 384 px wide; frame / sky / land) the
untouched neutral scores 23.5 / 37.0 / 12.4, the gate-on result under the old
vetoes 22.5 / 37.0 / 10.7, the previously shipped gate-off result 11.9 / 17.5
/ 7.4, and the ruled result 10.6 / 16.4 / 5.9 (segmentation off) and 12.5 /
20.6 / 5.9 (on). The Full-mode pairs (p36, viaduct) were byte-identical across that batch; the
per-band colour mixer has since moved both (p36 finished error
0.032592 -> 0.031792, viaduct look error 0.052060 -> 0.030419 with the
post-cast arbiter load-bearing). Repeated runs are SHA-identical. A 512 / 768 analysis edge was
measured and rejected: the same tiles, +4% / +25-50% wall time, and the
384-calibrated ruler collapses at 768.

**The local-field analyzer (B2, 2026-08-28).** Before any local producer runs,
`fit_zoned::field::solve_local_field`
([`src/fit_zoned/field.rs`](../src/fit_zoned/field.rs)) solves a read-only
12x8x8 bilateral field over the pair's shared analysis geometry
([`src/fit_field.rs`](../src/fit_field.rs)) and reads shape verdicts off it. It
is DISCLOSURE ONLY: the field is an owned local of `fit_recipe_zoned_inner`,
never a `FitReport` member, and a test greps `render.rs` and `recipe.rs` for
any mention of the module, so recipe schema era 1, the engine and XMP are
untouched and `src/fit.rs` needed no change at all. `LocalField::solve` returns
`None` when the objective already calls the pair unmeasurable (identifiability
<= 1e-5), when the fit weight carries no mass, or when the solve is
non-finite; a `None` field — like the disabled layer `ZonedLayerOpts { field:
false }` — leaves all three producers byte for byte as they were
(`field_disabled_layer_is_byte_identical`).

The single `run_local_sequencer` function owns the local-stage verdicts. The field is not threaded through
`attach_zones_with_divergence`; only two typed products cross into a producer
— the band proposals into `range::derive_luminance_bands` and the effective
attachment cap into `spatial::attach_tiles` — while `fit_recipe_zoned_inner`
itself reads the running `report.err_after` the producers already maintain (it
never recomputes it) and appends the realized and stop notes. `realized =
(global − err_after) / (global − ceiling)`, disclosed as `n/a` rather than
divided when the denominator is within 1e-6 of zero. When a producer already
lands within `LOCAL_STOP_MARGIN = 0.002` of the ceiling, the tile stage is
skipped and named — but only when `ceiling < global`: a field that saturated
or regularised its way ABOVE the producer-free frame measured nothing about
the headroom, is disclosed as such, and never vetoes a producer (the adversarial
review of 2026-08-28 found the unguarded rule would have suppressed the tiles
after any failed solve). Nothing else is appended on that path: every producer
already closes its own stage with `fit::append_finished_disclosure` on ITS
final render, so a skipped tile stage leaves exactly one finished disclosure,
and the persisted rationale string is never rebuilt from the
`MAX_NOTES`-bounded typed vector (which would truncate it and render the
truncation sentinel).

When a multi-region trial loses — or ties — arbitration, the two-region `FitReport` remains
intact and receives one typed `REGION_FRAME_REFUSED` note naming the trialled
class IDs, labels, and verdict keys. The losing rationale and masks are never
transplanted onto the winning recipe. The default two-region dials and
confidence remain byte-identical to `662b688`; the rationale gains one typed
`ZONE_ALREADY_MATCHED` note per zone whose dials did not move. A multi-class
layer that is unavailable, or that resolves no region past the support floor,
hands off to that same historical route under its own typed note
(`SEMANTIC_REGIONS_UNAVAILABLE`, `SEMANTIC_REGIONS_NONE`); the route's own
verdicts, anchor handling and sequencer apply unchanged. Both bridges size the
sidecar input through the one `segmentation_input` rule (native through
2048 px, thumbnail above), which is what the seeded run's identity rests on.

The verdicts are numbers, not prose. `BAND_DISPERSION_MAX = 15/255` separates a
luma bin a value band can describe from one that only varies in space: the
phase-A fixture sweep reads a spatially UNIFORM two-band edit at at most
9.2/255 (sparse, just-supported vertices included), spatially structured edits
at 21.9–51.8/255, and the calibration pair's re-rendered mid-tones at
28.7–29.1/255 — an order of magnitude apart, with 15/255 in the gap. Bin 0 is
excluded by construction rather than by measurement: every term of the
dispersion metric carries the bin's centre luma as a factor, so it is
identically 0 at `c = 0` and a band at pure black moves nothing either; the
disclosure says `0:blind` once instead of pretending to a reading. Surviving
bins merge while the luma effect of their parameter difference at the shared
boundary stays under 2/255, are capped at `RANGE_MAX_BANDS`, and must carry
`RANGE_MIN_EVIDENCE_SHARE` on both sides of the pair, both shares measured on
the same 3-tap guide luma. Shape is read off the remainder with every sum
weighted by the solve's own per-pixel fit weight (`LocalField.weight`: frozen
evidence x local support x unclipped), so a pixel whose vertices hold the
occupancy-floor policy zero — a missing measurement, not a measured zero —
cannot pose as spatial structure: `R2(4x4)` against the weighted 4x4 tile
means, `R2(linear)` against the weighted least-squares plane solved in f64.
Once the plane earns `LINEAR_SHAPE_MIN = 0.6` the tile figure becomes its
INCREMENTAL share over the plane's residual — otherwise a smooth ramp, most of
which coarse block means also capture, would be read as tile-shaped; an
incremental share cannot reach `TILE_SHAPE_MIN = 0.5` once the plane holds
0.6, so a `linear` verdict always carries the halved cap by construction.
Below `TILE_SHAPE_MIN` the quadtree's effective cap drops from
`SPATIAL_MAX_ATTACHMENTS` to two, and the calibration test pins that the
`TILE_DEPTH_CAP` note the quadtree prints equals the cap `LOCAL_SHAPE`
disclosed. Fixed in v1.2.4: under `structure_divergence`'s
100-core-pixel floor the reading ABSTAINS (`None`) instead of manufacturing a
matched verdict, and `local_support` gives an unread cell 0.0 — no support
claim — rather than the 1.0 of a measured match. A frame where NO cell
resolves keeps the old constant 1.0, deliberately: an instrument that can read
nothing anywhere must not starve a solve that used to run. Both halves are
pinned on a 190x128 frame that cuts sixteen-pixel columns (core 100, resolved)
and fifteen-pixel ones (core 90, abstaining) at the same time, by
`fit_field::tests::the_structural_reading_abstains_below_its_resolvable_core`
and `fit_field::tests::an_unreadable_cell_makes_no_support_claim_but_an_unreadable_frame_keeps_one`;
`calibration_local_support_is_not_constant` remains the reading's real-pair
check.

A band proposal is a span `[lo, hi)` of CURRENT-render luma — the field's
guide domain — and never an evidence-bin index: the range producer bins by the
ORIGINAL source luma (`evidence.source_pixels`), and after a global tone move
the two domains no longer coincide (the calibration pair's −1 EV global fit
shifts them by about two bins). `derive_luminance_bands` therefore maps each
span onto its own bins through the pixels that occupy it — the weighted
10th..90th percentile of their original luma
(`range::evidence_bins_for_span`, pinned by
`field_proposal_spans_are_mapped_through_the_pixels_that_occupy_them`, where
the naive index would have been two bins off). The mapped band enters AFTER
the evidence filter and BEFORE the sort and the cap, so it is judged and ranked
by exactly the rules a rank-paired band is: an overlapping opposite-sign
rank-paired run refuses it, a band whose own rank-paired residual disagrees
with the field's sign is refused as a disagreement (both typed abstentions), an
overlapping or adjacent same-sign run absorbs it and the merge note names why
(`absorbed by the overlapping rank-paired run before the cap`, distinct from the
cap merge's `after the four-band evidence cap`), and the existing
disjoint-sorted `debug_assert` in front of the cap still holds. On the
calibration pair no proposal survives its own gates (bins 3 and 4 are
structured, the rest fail the share or 2/255 magnitude line), so the union
changes no band and no dial — the analyzer's whole live effect there is the
disclosure and the smaller tile cap.

The final automatic layer is a frozen-evidence spatial quadtree
([`src/fit_zoned/spatial.rs`](../src/fit_zoned/spatial.rs)). It runs after
either semantic zones or the mutually exclusive luminance-range fallback and
always renders the current recipe before deriving residuals. A node intersects
the original pair's source and target evidence with its normalized rectangle;
edits cannot move pixels into evidence. Best-first priority is
`abs(signed_luma_residual) * min(source_share, target_share)`, with
`(depth,row,col)` as the deterministic tie-break. A child needs at least `0.03`
evidence share on both sides, original `D < 0.65`, a weighted 95% confidence
interval excluding zero, and a residual at least `2/255` away from its parent.
Eligible parents split only to `SPATIAL_MAX_DEPTH = 2` (4x4), accepted leaves
are re-derived after every attachment, and the stack stops at the analyzer's
effective cap (`SPATIAL_MAX_ATTACHMENTS = 4`, halved to two when the local
field reads the remainder as not tile-shaped).
Every examined ineligible node lands with its id and reason in that
generation's single typed sweep note (nodes already attached or refused told
their story in their own generation); eligible leaf candidates keep a full
per-node reading, and downstream failures (raster, estimator, boundary) keep
per-tile notes. The persisted-rationale abuse bound is 64 KiB (16 KiB before
v1.2.4) so this disclosure is never what truncation eats; the B3 free-mask
stage compacts its tentative attachment text before that bound, and typed
producer readings stay the retained disclosure.

The tile pass reads the pair through `fit::analysis_pair`, so its coverage
and estimator vectors are congruent by construction (asserted). Scoped tile
evidence is cached by `TileId` (v1.2.4): everything in a node's reading except
its residual is a function of the frozen evidence model and the tile's own
geometry, so the full-frame `scoped_mask_evidence` and the
`structure_divergence` behind it are computed once per node per fit instead of
once per node per generation. On the calibration pair the traversal reads 50
node evidences and computes 17 of them, and the cached reading is the same
reading a fresh one produces (`the_tile_evidence_cache_recomputes_each_node_once`).

Each leaf reuses `attach_one_zone` through `ZoneAttachment { min_share: 0.03,
frame_regression_tol: 0.0 }`, then passes its own `0.012` cross-boundary-step
and zero-regression composed-frame gates. Its deterministic normalized-source
raster is capped at a 2048-pixel long edge and persists as an existing
`MaskGeometry::Bitmap`, `MaskRole::Custom` adjustment. Recipe schema era 1 is
unchanged. Classic XMP deliberately skips the bitmap and returns the existing
named bitmap loss; no gradient approximation is emitted.

**Free-form remainder masks (B3, 2026-08-28).** After the quadtree tiles,
`fit_zoned::freemask` consumes only analysis pixels whose local-field remainder
still exceeds `SPATIAL_RESIDUAL_MIN` and whose accepted-tile alpha is below
0.5. Since v1.2.4 that alpha is the one the boundary gate LEFT the tile — its
raster times the accepted `k` — so a tile negotiated down to a sixth of its
dial reserves a sixth of its footprint instead of all of it, and what the
filter does remove is disclosed by number, pixel count and both shares instead
of silently never becoming a component. Deterministic 4-connected components
are sign-pure and ranked by `sum(abs(remainder) * LocalField.weight)`.
Components below the shared 64-pixel footprint floor (`MIN_MASK_PIXELS`, which
v1.2.4 also applies to spatial tiles) are refused before any render,
each must clear the shared 3% source/target evidence gate and
`structure_divergence < DIVERGENCE_ZONE`, with the two-proposal cap disclosing
all capped or otherwise refused components. Accepted components use the tile
upsample/refinement arguments and the exact shared frame/boundary gate (`0.0`
frame regression, `0.012` cross-boundary step). The B3 battery covers proposal, attachment,
disclosure, connectivity, cap, layer-off identity, ceiling stop, determinism,
bitmap recipe/XMP losslessness, neutral corpus disclosure, and p36 honest-
refusal checks. No live arm attached a free mask: downstream candidates were
refused by the shared gates. Tentative attachment text is compacted while
refinement and typed refusals remain; the B3 stage stays below 16 KiB, which
was the whole ceiling when it was written and an inherited pre-stage transcript
could reach exactly. v1.2.4 raised the ceiling to 64 KiB — one full note vector
is 16,183 bytes of TEMPLATE alone, so the old bound could be spent before a
single reading was written — and made truncation cut from the FRONT, keeping
the newest lines and stamping the marker that says how many bytes went.

Mask refinement is a production step, never a post-fit edit
([`src/mask_refine.rs`](../src/mask_refine.rs)). A dependency-free local-linear
guided filter uses integral-image box means, radius 8 and epsilon `(4/255)^2`.
It may propose semantic silhouettes and the boundary collar of an already
evidence-eligible tile, but never an observed-domain luminance range. Pixels
outside a `2 * radius` collar are restored byte for byte, whole-frame coverage
may change by at most `0.002`, and transition-weighted Sobel guide-edge
alignment may not decrease. Otherwise the original alpha is retained with a
typed abstention. A kept alpha is fitted from scratch and still crosses the
ordinary rim and composed-frame gates. Colour-range semantic regions remain
out of scope.

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
real failure pair. `AUTOSHADE_FIT_JOINT=off` removes the whole family for
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
construction. The CLI `match` keeps its embedded-rendition source and
post-fit stamp, but only while that rendition *is* the sensor frame: a body
set to an in-camera aspect writes a centred crop (a 4:3 preview over a 3:2
sensor), and since v1.2.2 the CLI detects it with the crate's one aspect
rule (`fit::same_frame_plausible_dims`, the 2 % tolerance the reference
check already used) and takes the GUI's composed route on a neutral
develop of the full frame instead — fitting on the crop paired the target
against a different frame, warned "CROPPED", and mapped every zone and
tile mask onto the wrong one. The same rule sizes `reimagine`'s request
from the frame it sends, and `render::camera_frame_of` pairs the base-look
estimator's develop against the frame the rendition shows. Fitting from the raw neutral spent the bounded model (the
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
per-user root that `AUTOSHADE_DATA_DIR` can move. The **delivery root** holds the
finished files, and before R24-5 it was not a setting at all: `./out`, spelled
literally in five places (the CLI deliverable name, the batch renderer's dedup
spelling, the pixel masters, the extracted style prompt, the web download
route), relative to whatever directory the app happened to be launched from —
which is precisely why "where did my export go" had no good answer.

`config::delivery_root()` is now the one reader, and `pipeline::default_out` the
one funnel every deliverable name is claimed through. It resolves the settings
file's `out_dir` over `AUTOSHADE_OUT_DIR` over the default `out`, memoised
(`unique_out` probes it up to 999 times per claim) and dropped by
`update_local_settings`, the one writer. An explicitly blank field is a real
choice — "the default" — and silences the environment variable too, the same
rule the two AI effort fields follow. The GUI exposes it in Settings beside the
develop store, echoing the RESOLVED absolute path, and its export destination's
「Delivery folder」 arm resolves through it, so this window, the CLI, the web
surface and a batch render name one place.

It is `Trust::Destination` in `config::SETTINGS`, unlike the read-only
`AUTOSHADE_LEGACY_OUT` beside it: a planted value does not merely choose a
folder, it decides where a stranger's developed photos are filed and (via
`guard_readonly`'s own-output allowance) which directory stops counting as the
read-only photo library. Neither a `.env` nor an ambient working-directory
`autoshade.local.json` may supply it.

Three things deliberately did NOT move with it, because they only ever shared
the folder NAME: the develop store (its own setting since v0.13), `out/imported`
where the web surface parks an uploaded SOURCE photo (library input, not a
deliverable), and `store::legacy_out_roots` / `AUTOSHADE_LEGACY_OUT` (a read-only
archaeology path pinned to where pre-v0.13 builds actually wrote, which a new
setting cannot retroactively change). `guard_readonly` keeps the literal `./out`
as an output area alongside the configured root, so repointing the root does not
make a `match` on an older export suddenly refused.

### 4.11 macOS: the bundle, the quit guard, and two spellings of one store

Three facts about a Mac break assumptions this app was built on, and each is
answered in one place rather than sprinkled through the call sites.

**⌘Q is not a window close.** AppKit terminates the process without ever
asking the window, so the unsaved-work prompt every other exit path goes
through was simply never reached. The guard installs
`applicationShouldTerminate:` on the delegate winit already owns, answers
`NSTerminateCancel` when the app is busy or dirty, and then asks the app to
run its ordinary close path — the SAME prompt, not a second one. The decision
itself is `quit::QuitState`, a pure function of three inputs (busy, the
user's already-given answer, unsaved work) with the expensive walk of the
variant strip behind a closure, so the two cheap inputs decide without paying
for it. It is compiled and unit-tested on every platform; only
`class_addMethod` is `#[cfg(target_os = "macos")]`.

**A signed bundle is read-only.** Inside `<App>.app/Contents/` the sidecars
ship at `Resources/python/`, so `config::bundled_helper` stops its ancestor
walk AT the bundle instead of climbing out of it into `/Applications`. Model
weights must NOT land there: writing into a signed bundle invalidates the
signature and Gatekeeper refuses the NEXT launch, so `default_weights_dir`
sends them to the develop store instead. That override travels as one
`--cache` argument appended at the three spawn points, and it is appended only
when it DISAGREES with the sidecar's own script-relative default — an
unconfigured Windows install spawns the argv it always did, byte for byte.

**The store root is spelled differently per platform, on purpose.** The
develop store is `Library/Application Support/AutoShade` on macOS, because
that directory holds applications' display names, and lowercase `autoshade`
elsewhere, because those roots hold existing users' data and renaming a
directory named by a hash of an absolute path orphans every develop under it.
For the same reason the one-time adoption of a pre-rename `autoshop` store is
DISABLED on macOS: no Mac ever ran the old name, APFS is case-insensitive by
default, and an adoption firing there could only rename a directory belonging
to somebody else's program. Both spellings are pinned by a test that asserts
the arms it cannot execute still EXIST in the source — deleting the macOS arm
is otherwise a silent no-op on the machine the battery runs on.

The opt-out is stronger than a source assertion. `adopt_pre_rename_root` takes
the decision as a PARAMETER, the shape `adopt_prefs_between` next door already
used, and both production call sites pass the constant; so the refusal runs as
a BEHAVIOURAL test on every platform — that a directory this build does not own
keeps every byte — and the source check is reduced to confirming those two call
sites pass the constant rather than `true`. Reading the constant inside the
function instead left three of the four branches unreachable on macOS while the
tests describing them asserted unconditionally, which is why the Mac lane failed
on exactly those three for every push after the port; and it left the refusal —
the one branch macOS could take — executed by no test on any platform.

The GUI's own preferences (window size, theme, language) use the eframe
storage key `AutoShade` on every platform. The pre-rename `Autoshop`
roaming-profile folder is adopted at launch on the store's own doctrine
(rename wholesale; on failure keep the LEGACY key for the session so nothing
resets, and retry next launch; both-exist keeps both and uses the new one) —
changing the key alone would have silently reset every current user's window.
The same C2 ruling moved the per-output export registry from
`.autoshop-export-registry` to `.autoshade-export-registry`: the directory is
renamed wholesale on first use, so every claimed deliverable suffix survives,
and a failed rename falls back to the legacy namespace rather than
reassigning filenames. macOS opts out of the prefs adoption for the store's
reason (no Mac ever ran the old name; case-insensitive APFS).

The SETTINGS FILE follows the same doctrine and is the last piece of the rename
to do so. v1.2.0 answered to a second spelling forever — every `AUTOSHOP_*`
environment name aliased its `AUTOSHADE_*` twin, and the loader read
`autoshop.local.json` as a permanent fallback — which meant a v1.1 upgrader's
API keys lived in a file the app would read but never write, so the next save
wrote the new name and the old one silently kept a stale key. v1.2.4 closes
both: the alias door is gone (no `AUTOSHOP_*` name resolves anywhere), and the
pre-rename file is RENAMED ONCE on first load, exactly like the store root — a
same-volume `rename`, both-present keeps both and uses the new one, and a
failed rename falls back to READING the old file for that session so nothing is
lost while the rename is retried next launch. The adoption is latched, so it
answers once per process rather than per settings read.

`scripts/build_app_bundle.sh` assembles the bundle: both binaries, the
sidecars, an `.icns` rendered from the shipped PNG, a hand-written
`Info.plist` (`plutil`-linted, its minimum system version checked against the
deployment target the binaries were actually built for), and an inside-out
ad-hoc `codesign`. Ad-hoc signing is a real limitation and is documented as
one in the README: it costs the user one 「Open Anyway」, and notarisation is
not scheduled.

### 4.12 Windows: one AppId, and an uninstall the user answers

Two promises live in `installer/autoshade.iss` — setup upgrades an existing
install in place, and the user can choose to uninstall — and both are
behaviour, not settings, so both are checked by installing.
`scripts/installer_scenarios.ps1` drives the installer through the six states a
user can put it in (fresh install, upgrade over a running program, downgrade,
uninstall keeping the data, reinstall, uninstall deleting it) and asserts what
each one leaves in the registry, on `PATH`, in the Start Menu and on disk.
`.github/workflows/installer-upgrade.yml` runs that same script on a clean
Windows runner against the PREVIOUS published release, so the upgrade is
measured across the version boundary the release notes claim it works over, and
not against a second copy of the same build.

**Upgrading is the same install, moved forward.** `AppId` is a constant GUID
that already survived the Autoshop rename, and it is what Inno recognises an
existing install by, so it is never derived from the version;
`UsePreviousAppDir=yes` then puts the new files where the old ones are,
whatever directory that is. What comes out the other side is one Programs and
Features entry, one `PATH` entry, the same three Start Menu shortcuts
overwritten rather than added to, and every shipped file replaced
(`ignoreversion`) — while `{app}\python\weights\`, the multi-gigabyte
download the sidecars fetch on first use, and the develop store outside `{app}`
are left byte for byte alone, because neither is installer payload. A running
AutoShade holds its own executable open, so `CloseApplications=yes` lets
Restart Manager close it and `RestartApplications=no` declines to start it
again; the log names what it found. Going BACKWARDS is refused: setup reads
`DisplayVersion` out of the uninstall key and compares it NUMERICALLY — a
text compare puts 1.2.10 before 1.2.9 — then stops with a message naming
both versions, which in a silent run is a non-zero exit code and the reason in
the log.

**Uninstalling has two doors and one question.** Programs and Features carries
the entry Inno writes from the AppId, and the Start Menu group carries
「Uninstall AutoShade」 beside the two launchers, so a user who never
opens that control panel still has a way out. Either door asks whether to
delete the two things the installer never installed — the downloaded
weights and the develop store — naming the size found in each and
defaulting to keeping both. A silent uninstall cannot be asked, so it keeps
them unless `/DELETEDATA=1` is on its command line. That question is put with
`SuppressibleMsgBox` rather than `MsgBox`, which is the difference between a
decision and a hang: a plain `MsgBox` from Pascal Script displays even under
`/VERYSILENT /SUPPRESSMSGBOXES` and waits there for a click nobody is present
to give. Program files, shortcuts, `PATH` entry and registry entry go in either
mode; the install folder itself goes only when the data went with it.

A scenario run must never be able to touch the real install, so the identity is
a compile-time parameter: `/DAppIdOverride` and `/DAppNameOverride` give a test
build its own GUID, its own Start Menu group and its own installer-state key,
and a release compile passes neither and therefore always gets the constants.
The scenario script asserts that, and photographs the shipped install's
registry entry, directory and Start Menu folder before and after to prove they
did not move.

## 5. Why Rust — and the whole stack, named

Cross-platform, no GC pauses on large-image pipelines, first-class image crates,
single-binary distribution, trivial `std::process` shell-out to `claude`.
Toolchain in use: rustc/cargo **1.94.1**, **edition 2024**. Since v1.2.4 that
version is PINNED rather than observed — [`rust-toolchain.toml`](../rust-toolchain.toml)
carries the channel, `rustup` honours it for every `cargo` call in the
repository, the release and build workflows assert that the runner got it
before they compile anything, and `scripts/check_docs.py` compares this
sentence against the pin, patch included. Before that the sentence said
"verified locally" and meant it: one laptop, one day, while CI built the
published binaries with whatever `stable` was that morning.

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
| `brotli-decompressor` 5.0.3 | bounded decompression of modern Lightroom `MaskBrushTable` objects from the sibling ACR store; pinned to the version already present through jxl-oxide |
| `bytemuck` 1 | zero-copy `Vec<[f32;3]>` ↔ `Vec<f32>` casts in the orientation stage — a 61 MP portrait RAW otherwise pays three ~732 MB full-frame copies (`render::orient_f32`) |
| `clap` 4.6.1 (`derive`) | the CLI surface: subcommands, `--jobs`, `--strength`, the rest |
| `dotenvy` 0.15 | reads `.env` — under the trust table of §3, which is why a `.env` may carry a `Secret` and not a `Destination` |
| `getrandom` 0.3.4 | CSPRNG bytes for the `serve` session token gating image URLs; anything seeded from the clock is guessable, which is the whole attack. Already transitive, so no new dependency |
| `image` 0.25 | baked-source decode + every export encode. `default-features = false` and the codec set is opt-in one at a time — `jpeg`, `png`, `tiff`, `webp`, `bmp`, `gif`. avif/heic stay OUT because they mean a C toolchain (dav1d) this tree does not have; R27 added the last three only after checking each one's dependency closure (all pure Rust, no `build.rs`, no bundled C) |
| `md5` 0.8.0 | verifies the content-addressed key of an ACR `MaskBrushTable` object before parsing it; integrity only, never authentication, and pinned to the version already present through rawler |
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
surface. macOS adds NO crate at all: free memory is `libc`'s
`sysctlbyname("hw.memsize")` plus `host_statistics64`, and the ⌘Q guard
([`src/bin/gui/macos.rs`](../src/bin/gui/macos.rs)) reaches AppKit by linking
`libobjc` directly and installing `applicationShouldTerminate:` on winit's
existing delegate at run time — the same raw-`#[link]` shape the Windows
message box already used, chosen over an `objc2`/`cocoa` dependency because
the port needs exactly one selector. The decision that guard reports is a
portable state machine compiled and tested on every platform
([`src/bin/gui/quit.rs`](../src/bin/gui/quit.rs)); only its delivery is
Darwin-only. Build-time: `winresource` 0.1 embeds the Windows app icon
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

**The Python sidecar stack** is deliberately NOT in `Cargo.toml`: five scripts
under `python/`, shelled out to with `-E`, each doing one job and each failing
loudly rather than degrading silently (`lib.rs::sidecar_wrote`). They need
Python 3 + PyTorch (CUDA where the box has it), plus `transformers` on the sky
and object paths and `rembg` on the subject path. Which device each one runs on
is one shared answer rather than five:
[`python/_device.py`](../python/_device.py) resolves `cuda` -> `mps` -> `cpu`,
so an Apple-silicon Mac reaches the GPU through Metal without any script
deciding for itself. The CUDA spelling is a PARAMETER of that helper precisely
so the CUDA argv it produces is unchanged from before it existed. The rest of
what being a sidecar means — logging, refusing, the pinned checkout, the
verified fetch, the atomic publish — is likewise one answer rather than three,
in [`python/_sidecar.py`](../python/_sidecar.py). The
dependency sets that differ per platform are split into
`python/requirements-{common,cuda,macos}.txt` — the CUDA file is the only one
carrying an extra index URL, and the macOS file installs plain PyPI wheels
because Metal support ships in the default build. The five models they load,
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
| 2 | Camera / RAW format | ~~resolved: Sony `.ARW`~~ **← R27 (2026-08-19) widened**: 24 RAW extensions (`decode::RAW_EXTS`) + 8 baked (`pipeline::BAKED_EXTS`), one predicate app-wide; 9 cameras (one per format) verified end to end on CC0 samples, and the zoo is a release gate (`AUTOSHADE_RAW_ZOO`, 9/9 at v0.33.0); Adobe DNG Converter is the documented on-ramp for the rest. Refusals are named per cause (unknown make / unknown model / no decoder); monochrome and 4-colour sensors are refused before the develop; X-Trans develops approximately and says so |
| 3 | Output target | resolved: XMP sidecar **+** rendered, XMP-first |
| 4 | AI roles | resolved: GPT=image, Claude=non-image+verify, unified framework |
| 5 | Exact meaning of Claude's "收货验证" (data-level vs pixel-level) | resolved: **data-level**. The verifier is never sent pixels — it judges the recipe against EXIF/histogram/clipping stats and the advisor's rationale (§3, §4.3, [`src/advisor/claude.rs`](../src/advisor/claude.rs)) |
| 6 | How to feed the preview to the GPT vision API; `crs:` key set for ARW | resolved in shipped code: the preview goes as a base64 `input_image` data URL on the Responses API with a strict `json_schema` ([`src/advisor/openai.rs`](../src/advisor/openai.rs)); the ARW `crs:` key set is the one the writer emits and round-trips ([`src/xmp.rs`](../src/xmp.rs)) |
