# AutoShade v1.8.1 — rustls 0.23.45

A dependency update and nothing else. The TLS library behind every HTTPS
request AutoShade makes (the generative fill and AI edit, the AI advisor,
reimagine, the model list) moves from rustls 0.23.41 to 0.23.45, and its
certificate verifier rustls-webpki from 0.103.13 to 0.103.15 — GitHub's
dependency bot proposed it (#16) and the maintainer chose to merge it and ship
it as a patch on 2026-10-13. Only `Cargo.lock` changes; no AutoShade source
file, recipe field, sidecar or document behaviour changes, and every recipe
renders as in v1.8.0.

## Compatibility

Recipes, stores and sidecars are exactly as in v1.8.0. The sidecar weights
stay pinned to the v1.6.0 `autoshade-raw-denoise-v2.pth`; no weights are
re-shipped.

## Gates

Scoped to what the change touches (the maintainer's ruling of 2026-10-13:
run the gates for the changed parts only). The library tests of the five modules that make HTTPS
requests (`advisor`, `config`, `generative`, `openai_models`, `retouch`):
**180 passed / 0 failed / 2 ignored**; clippy 0 warnings on both feature sets
(`--all-targets -- -D warnings`); and one live handshake through
`openai_models::list_models` with a deliberately invalid key, built against
this `Cargo.lock` (rustls 0.23.45): the server answered `401 invalid_api_key`,
so the TLS connection itself was established and verified. The dependency
bot's own CI run on the change was green, and the push and release runs
rebuild and test both feature sets on every platform. After the version bump the CLI and
contract suites read 28 / 0 and 2 + 2, `check_doc_drift` 0 findings and
`check_docs.py --gates` 32 PASS / 0 FAIL / 0 SKIP (the test counts read off
v1.8.0's battery transcript, which this change leaves standing).

Not re-run, because nothing they cover changed: the full three-lane battery,
the GUI lane, the calibration lane, the Python suites and the reference-pair
final gate (v1.8.0's readings stand); no new invariant was added, so there is
no mutation to falsify.
