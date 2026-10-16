# AutoShade v1.10.0 — the image role on any OpenAI-compatible server, the model list read for what it says, and the app's words reviewed

Everything merged into `main` since v1.9.0 ships here. The image role — the
AI that looks at the photo and proposes, describes, judges and finds spots —
now works against a server that serves only `/chat/completions`, which is most
OpenAI-compatible servers: local runners, relays and the other vendors'
compatibility layers. Where a `/models` listing says which reasoning tiers a
model takes and whether it sees images (Venice, OpenRouter), the pickers offer
that instead of guessing. Twenty-five strings of the desktop app were
rewritten after a read-through for the words a new user cannot know, and a
long error toast wraps instead of running past the window.

The design, the code and this document are the maintainer's; no external
coding model wrote this release.

## The image role over Chat Completions

- **What changed.** The image role's four vision calls — the proposal, the
  style description, the visual judge and the retouch spot finder — each
  wrote a Responses-API body and posted it to `/responses`. A base URL and a
  key of a server without that route worked for the analysis role (chat
  completions since its start) and failed the image role with a 404. One
  description of a vision call is now spelled for either family: the
  Responses body byte for byte what was sent before, or a Chat Completions
  body with `image_url` parts and `response_format.json_schema`.
- **API style.** A new setting, **Settings → Image → API style**
  (`image_api_family` in `autoshade.local.json`, `AUTOSHADE_IMAGE_API_FAMILY`
  in the environment; the browser page reads and writes it too), with three
  values. **Auto**, the default, tries `/responses` and, when the server
  answers 404 or 405 there, sends the same call to `/chat/completions` and
  remembers that for the endpoint until the app closes. **Chat Completions**
  and **Responses API** pin one shape. Only those two statuses switch: a 400
  is the endpoint objecting to a parameter, negotiated knob by knob inside
  the family; a 401 or 429 is the key or the quota, which no other shape
  would cure.
- **The structured-output contract joins the negotiated knobs.** A 400
  naming `response_format` (or `text.format`) is answered once without it,
  and the reply is read as the JSON the prompt asked for. The extractors read
  the Chat reply shape — `choices[0].message.content` as a string or as text
  parts, `message.refusal` — beside the Responses one.
- **Not changed.** Generative edits (fill, the whole-image generation) still
  need `/images/edits` with a mask, which only some providers serve; the
  manual says so beside the setting.

## The model list, read for what it says

- `GET /models` gave the pickers ids only; the effort picker offered the
  same three tiers for every model, and the vision picker could not say that
  a model takes no image until the first paid call refused it. Some listings
  say both: Venice's `model_spec.capabilities` (the reasoning tiers a model
  takes, whether it sees images), OpenRouter's `architecture` and
  `supported_parameters`.
- **Fetch models** keeps those beside the ids, cleared with them. The
  **Reasoning effort** picker of either role offers the chosen model's own
  tiers where known, with a note saying they came from the listing — or that
  the model takes none, and the field is left blank. A vision model the
  listing says takes no image is flagged beside its picker. The plain OpenAI
  listing says nothing, and there nothing changes: the fixed suggestions
  stand in.

## The desktop app

- **The reviewed strings.** The 25 items of the UI text review of
  2026-10-16, the words a new user cannot know replaced with what the thing
  does. The Settings headings name the roles by the buttons that use them
  (*Analysis model — used by AI Analyze, AI Refine and AI review*; *Image
  model — looks at your photo, and generates or edits images*). The Codex
  bridge is **Sign in with ChatGPT (via the local Codex bridge)**, its token
  the **Bridge password**, and the note under it — and the manual — say how
  to install and start CLIProxyAPI. Reverse-fit is introduced as matching a
  finished look (**Match a look (reverse-fit) · local**); its *Zoned fit
  (sky)* switch is **Fit local areas too**, with a hover in plain words;
  *deep* is **Retry once**; the strength, adherence and reverse-fit strength
  hovers say what the dial does instead of how it was calibrated. *Fold* is
  *section*, *baked* is *already-rendered*, the Python sidecars are
  *helpers*, the three "this build did not ship" messages say what to
  install, the hard-coded `./out` is *the delivery folder*, Reimagine is
  **Generate whole image** everywhere, the fit's status lines say
  *difference from target* and *mood-only match*, the verdict line says
  whose verdict it is, and the two heuristic-baseline rationales say what
  happened and where to look. The Chinese text names the Chinese labels and
  uses one word per concept.
- **Not done:** the engineering-log explanations of the fit (the review's
  closing suggestion of a plain summary with the details behind a toggle).

## Fixed

- **A long toast ran past the window.** The toast label sits in a child of a
  right-to-left row, and egui 0.29 gives a label in a horizontal layout the
  Extend wrap mode, so a long error text ignored its 420 px measure and drew
  as one line off the window's edge. The label asks for Wrap now; the
  measure, the slide-in and the fade are unchanged.

## Compatibility

- **Settings.** One new key, `image_api_family`, default `auto`; a settings
  file without it behaves as before against a server that serves
  `/responses`, and gains the fallback against one that does not.
- **Nothing else moves.** Recipes, stores, the reference library, the command
  line and the browser page's routes are those of v1.9.0; the sidecars under
  `python/` are byte for byte v1.9.0's, and the weights stay pinned to the
  v1.6.0 `autoshade-raw-denoise-v2.pth`.

## Gates

Measured before the tag on the release code (`a2019ed`). The version bump
touches Cargo.toml, Cargo.lock, the documents, the site's cache keys and the
bug template's dropdown, and the CLI, contract and doc-test suites, clippy,
`check_docs`, `check_doc_drift`, the i18n audit and the font check are re-run
after it. The three-lane release battery (`scripts/release_battery.sh`, a
frozen snapshot worktree, the p36–p41 calibration corpus and the sidecar
weights in reach): library **1633 passed / 0 failed / 15 ignored** (768.19 s,
release profile, one process per module), CLI **25 / 0**, contract 2 + 2,
doc-tests 0, GUI **241 / 0 / 3**, calibration lane **1633 / 0 / 15**
(1039.20 s; one environment-gated probe skipped for the unset brush-sample
root, as in the v1.9.0 transcript); `audit_i18n`, the font check (897 of 898
code points embedded, one left to egui's bundle) and `check_doc_drift`
(0 findings over 14 documents) exit 0 inside the battery. clippy 0 warnings on
both feature sets (`--all-targets -- -D warnings`). `check_docs.py --gates` on
the transcript with the XMP census root supplied: **32 PASS / 0 FAIL /
0 SKIP**.

By name against the v1.9.0 release battery: library 1643 → 1648, **+5 / −0**
— four in the OpenAI provider for the image role over Chat Completions (the
fallback from a missing `/responses`, the learned family not probed again, an
explicit Chat family that drops a refused contract, the extractors reading the
Chat shape) and one in `openai_models` for the tiers and the vision flag a
listing names. GUI 243 → 244, **+1 / −0** (the listing's capabilities reaching
the pickers and clearing with the ids). CLI 25, unchanged.

The first battery run, on `c633897`, passed every lane and failed the i18n
audit: the API-style picker names its three values from one table the audit's
registry did not know, so two of the labels had no Chinese entry and the third
matched no call site. The table is registered and the entries added
(`a2019ed`), and the battery above is the second run, on that commit.

The Python suites were not run: `python/`, `scripts/` and `tests/` are byte
for byte v1.9.0's (the ruling of 2026-10-13: run what changed). Final gate,
reference pair: not re-run. The reverse fit and the develop are untouched —
the batch changes the advisor's transport, the desktop app and the documents —
and a recipe renders as in v1.9.0.

Mutations falsified the new invariants on the release code, each red and each
restored (SHA-256 equal): the fallback keyed to a 418 instead of a 404 or 405
turned `a_missing_responses_route_falls_back_to_a_chat_completion` red; the
learned family never written turned
`a_learned_chat_endpoint_is_not_probed_again` red; a refused
`response_format` never recognised turned
`an_explicit_chat_family_posts_chat_first_and_drops_a_refused_contract` red;
the extractors reading `choicez` turned
`the_extractors_read_the_chat_completions_shape_too` red; a listing that says
"no tier" read as "said nothing" turned
`the_listing_says_which_tiers_and_images_a_model_takes_when_it_says_anything`
red; the capabilities kept across a clear turned
`the_listing_capabilities_reach_the_pickers_and_clear_with_the_ids` red; and a
fourth API family added to the picker's table without a Chinese entry turned
`scripts/audit_i18n.py` red. The toast's wrap has no test: it is a layout
request to egui, looked for in no assertion.
