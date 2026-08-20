<!--
Thanks for the patch. The checklist below is not ceremony — it is the core of
the battery every release runs (releases additionally run three env-gated
fixture suites; see the last Gates row), and each line exists because skipping
it has broken something here before.
-->

## What this changes

<!-- One paragraph. What was wrong or missing, and what the change does about
it. Link the issue if there is one. -->

## Why this, and not something else

<!-- If you considered another approach, say so in a line or two. If this is a
render change, say whether the same recipe now produces different pixels — that
is a real compatibility boundary in this project, not a detail. -->

## Gates

Both build configurations, every time. The GUI sits behind the `gui` feature, so
a plain `cargo test` never compiles it — a change can be green on one
configuration and broken on the other.

- [ ] `cargo clippy --all-targets` — clean
- [ ] `cargo clippy --all-targets --features gui` — clean
- [ ] `cargo test` — green
- [ ] `cargo test --features gui` — green (the FULL form, not `--bins`: the
      library and the two contract suites must also pass under the `gui`
      feature, and the `--bins` form cannot even express that)
- [ ] **Touched decode / XMP / masks?** Run whichever env-gated fixture suites
      you have — `AUTOSHOP_RAW_ZOO` (full test name
      `every_make_in_the_raw_zoo_decodes_and_agrees_with_itself`),
      `AUTOSHOP_LR_PROBE_FIXTURES`, `AUTOSHOP_MB_FIXTURES` — and say in the PR
      which ones you could not run. Unset, they skip silently.
- [ ] **Docs changed?** `python scripts/check_docs.py` prints **PASS** on every
      row it can derive. That gate re-derives the hard numbers in the docs from
      the tree, and a claim site it can no longer find is a **FAIL**, not a
      silent skip — if you rephrased a sentence it was anchored on, re-anchor
      the row.
- [ ] **No `cargo fmt`.** This tree is hand-formatted and has no
      `rustfmt.toml`; running it rewrites tens of thousands of lines and buries
      the actual diff. `cargo fmt -- <file>` is not a file filter — it
      reformats the whole crate anyway. Match the style of the code around you.
- [ ] **Line endings preserved per file.** Endings are mixed here by design and
      `.gitattributes` pins `*.py` to LF, because `denoise.rs` `include_str!`s
      `python/denoise.py` and asserts on literal source. Do not let an editor
      normalise a file you did not otherwise touch — check `git diff --stat` for
      files that are suddenly whole-file changes.

## If this touches a degradation

Autoshop's rule is that nothing degrades quietly: every loss has a name, and
that name reaches the CLI's stderr, the desktop app's banner and the web reply.

- [ ] Anything now carried-but-not-rendered, approximated, or refused is
      **disclosed by name** on every front-end that can reach it — or this PR
      does not add such a case.
- [ ] Any new user-visible string is in both languages and passes
      `python scripts/audit_i18n.py`; new 中文 text is covered by the GUI font
      subset.

## Notes for the reviewer

<!-- Anything you are unsure about, deliberately left out, or want argued with.
A named open question is worth more here than a confident summary. -->
