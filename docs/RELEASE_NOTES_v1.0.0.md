# Autoshop v1.0.0 — the first major release

Autoshop v1.0.0 turns the post-v0.35.0 Lightroom measurement work into a
clearer mask-frame contract, improves modern sidecar import, and makes the
project documentation and release checks harder to drift. The deterministic
develop path remains recipe-based: AI proposes or derives intent, while the
Rust renderer owns the pixels.

## What changed since v0.35.0

### Rendering fidelity

- Angled LINEAR masks now evaluate their projection in the frame's pixel/aspect
  metric instead of normalized-coordinate distance (`ecb6505`). On the measured
  9504×6336 probe, the half-contour error fell from 874 px to 9.8 px;
  axis-aligned gradients and square frames remain byte-stable.
- RADIAL mask transport now uses Sony 0x7037's measured `(i+1)/16` native
  radius law, a metadata-derived full-raw-frame centre, a 64-knot persisted map,
  and the exact-once `m_lr^-1 ∘ T_engine` sampler (`706ac84`). All 41 measured
  radial point vectors close to at most 1 px.
- LINEAR masks now use their measured H2 topology instead of sharing RADIAL's
  pointwise sampler (`ad6de62`). With correction enabled, Autoshop reconstructs
  the straight gradient in the corrected frame. With correction disabled, it
  transports only the Zero/Full handles and rebuilds one straight line in the
  raw pixel metric.

### Lightroom interoperability

- Camera-metadata lens profiles now preserve the mask-warp centre as
  `LensProfile.mask_warp_center`; disabled-sidecar LINEAR transport preserves
  its separate camera map as `LensProfile.linear_handle_warp`.
- Modern Lightroom `MaskBrushTable` data can be read from the companion `.acr`
  content store with strict directory, digest, envelope, Brotli, and payload
  validation (`2cb59a5`). Supported table-encoded brushes now import and render
  instead of producing the previous named out-of-model refusal.
- Unmodified table-backed mask groups remain byte-preserved on sidecar merge.
  Unknown or malformed table forms are still refused by name rather than
  guessed or partially imported.
- The lens-profile refusal model remains explicit: unsupported fisheye LCPs,
  unparseable profiles, and missing profile roots do not silently invent a map.

### Masks and local AI

- Gesture dabs attached to subtype-0 object masks now become ordered positive
  SAM 2.1 point prompts (`1e99e84`). Prompt transfer uses a bounded temporary
  JSON file rather than an unbounded command line.
- Only subtype-0 masks that carry gestures receive the new `gp1` cache identity
  and re-derive once. Subject masks, sky masks, and gesture-free object masks
  retain their previous cache keys and behavior.
- Object/background intent remains ambiguous in Lightroom's subtype-0 carrier;
  Autoshop continues to disclose that it locally re-derives an alpha rather than
  importing Adobe's unavailable raster.
- The training-data scope comments were audited and corrected (`70832eb`):
  Autoshop distributes neither ADE20K nor SA-1B, while the downloaded
  OneFormer and SAM runtime artifacts remain governed and digest-gated under
  their own MIT and Apache-2.0 terms.

### Application and evaluation

- A new per-user Windows installer needs no administrator access, creates Start
  Menu shortcuts, offers optional desktop and user `PATH` tasks, and preserves
  the develop store on uninstall.
- Evaluation rows with fewer than 20 observations are marked `[low n]`, and a
  supplementary n-weighted gap is reported without changing the established
  headline, state-file, or n≥20 definitions (`ef5a71a`).
- Retry disclosures now distinguish native Anthropic 529 refusals from relay
  failures that may have completed upstream, and warn that retrying relay
  524/529 failures can incur a second charge.
- Advisor error bodies use the same bounded read policy as the other response
  paths. Store safety tests now use explicit temporary roots rather than a real
  user data location.
- A same-version rerun measured a 16.3% evaluation gap versus the 17.7%
  baseline. That 1.4-point resampling swing is comparable to the prior
  1.7-point cross-version change, so the 17.7% figure is not evidence of a
  rendering regression.

### Documentation and release gates

- The README is now a structured install guide, user manual, CLI reference,
  Lightroom/AI explainer, and measured showcase (`53bb77f`).
- Architecture and planning docs now state the RADIAL and LINEAR frame laws,
  schema breaks, precision limits, and the current test battery.
- `scripts/check_docs.py` now checks the exact README RAW-extension membership,
  the exact no-preview format membership, and README/ARCHITECTURE battery
  consistency in addition to the existing version, count, dependency,
  toolchain, camera, and corpus checks.

## Hard changes and compatibility

v1.0.0 remains backward-compatible with older recipes: both new lens-frame
fields have defaults, so recipes written by earlier releases remain readable.
Forward compatibility is deliberately strict. Older executables use
`deny_unknown_fields` for `LensProfile` and therefore refuse a v1.0.0 recipe
that carries either `mask_warp_center` or `linear_handle_warp`. Refusal is safer
than silently discarding a coordinate transform and rendering the mask in the
wrong frame.

The following existing content can render differently in v1.0.0:

- angled LINEAR masks on non-square frames, because their metric is now
  pixel/aspect-correct;
- RADIAL masks paired with camera-metadata lens profiles, because the native
  knot law, map centre, and exact-once transport are now applied;
- LINEAR masks paired with camera-metadata lens profiles, in both the
  correction-on and correction-off paths, because LINEAR now follows H2;
- modern table-encoded Lightroom brushes, which move from named refusal to
  actual brush rendering; and
- subtype-0 object masks with gesture dabs, whose SAM prompt set and scoped
  cache identity now include those dabs.

No released Autoshop version shipped the intermediate `706ac84` LINEAR
behavior; `ad6de62` is the v1.0.0 LINEAR contract.

## Known limitations

- RADIAL point transport is 41/41 at ≤1 px, but the R2 large-mask dilation has
  an open residual of about 1.2 percentage points. Clean cells are within
  0.35 pp and R1 is about 0.5 pp.
- LINEAR H2 is not 1 px-closed. Correction-on residuals are
  9.748/7.025/6.336 px RMS; correction-off residuals are
  12.449/9.943/4.979 px RMS. A fitted anisotropic-aspect term remains diagnostic
  only and is not shipped.
- AI masks are local re-derivations, not Adobe raster imports. Model or sidecar
  failure leaves the adjustment skipped and disclosed rather than applying an
  inverted mask to the whole frame.
- The X-Trans path is a geometry-aware approximation, not a
  Markesteijn-class directional demosaic.
- Generative reimagine/retouch outputs are lossy, resolution-limited targets,
  not full-resolution deliverable masters.
- Prebuilt artifacts are Windows-only for this release; Linux and macOS are
  built and tested in CI but remain less exercised interactively.

## Validation

The v1.0.0 source battery is 871 library tests (862 pass + 9 ignored forensic
probes), 14 CLI tests, 132 GUI-feature tests, and two integration suites with
2 tests each. The GUI-feature Clippy and compile gates are clean.

## Checksums

| Artifact | Size | SHA-256 |
|---|---:|---|
| `autoshop.exe` (CLI) | 31,180,152 bytes | `116a38410a810b1b27602c97daa4db614241b89fffbb80c6691a275fc7f168c0` |
| `autoshop-gui.exe` (desktop app) | 40,810,704 bytes | `847f42c4b35c09ab5dd040fdf8e90f99d597c66624ef131ac02d93071bcb58ce` |
| `Autoshop-Setup-1.0.0.exe` (installer) | 19,768,387 bytes | `28c4acd37089e78bf02182cd8b20a214a63cababb1b02971209be3fdf33d4750` |
| `autoshop-1.0.0-windows-x64.zip` (portable archive) | 27,131,443 bytes | `47389ed42f80798ead96980d69ce10f5063ece606e0f0d548482c58aef9f717e` |
