# AutoShade v1.11.0 — generative edits through any provider's edit call

Everything merged into `main` since v1.10.0 ships here. Generative edits — the
painted-area fill, the edit without a painted area and the whole-image
generation — posted only to OpenAI's `/images/edits`. A provider whose edit
call is shaped differently answered 404: Venice, for one, serves the vision
calls v1.10.0 taught the image role, but edits at `/image/multi-edit`, with a
JSON body of base64 images and the picture itself as the reply. Settings now
offers two choices for that call, **OpenAI standard** and **Custom**, and the
custom call is a template the user writes for their provider.

The design, the code and this document are the maintainer's; no external
coding model wrote this release.

## The custom image-edit call

- **Where.** **Settings → Image → Image-gen API**: **OpenAI standard
  (/images/edits)**, the call every earlier release made, or **Custom**, which
  opens a box for a JSON template (`image_gen_endpoint` in
  `autoshade.local.json`, `AUTOSHADE_IMAGE_GEN_ENDPOINT` in the environment;
  the browser page's settings route reads and writes it too). The empty box
  shows an example to start from, a line under it says whether the template
  is valid, and an empty template saves as OpenAI standard. There are no
  provider presets: a provider is one user's template, read off that
  provider's own API reference.
- **The template.** `"path"` is added to the image role's base URL and sent
  with the same key; `"body"` is the JSON sent, with `{model}`, `{prompt}`,
  `{image}`, `{image_data_url}`, `{mask}`, `{mask_data_url}`, `{marked}`,
  `{marked_data_url}`, `{marked_note}`, `{width}`, `{height}`, `{size}` and
  `{quality}` filled in; `"reply"` is `"image"` when the answer is the picture
  itself, or a JSON pointer to its base64 (default `/data/0/b64_json`). A
  placeholder in the user's own words is text, not expanded again; `{width}`
  and `{height}` alone in a value become numbers.
- **Telling a provider with no mask parameter where.** An empty prompt over a
  painted area means *remove what is there*, and the model learns where from
  the mask — which a provider such as Venice has no field for. `{marked}` is
  the picture with the painted area tinted red, sent as another image, and
  `{marked_note}` tells the model to change only the red area and leave no
  red. `{mask}` is the area in black and white (white = change) for a provider
  that does take one. All of them are left out when nothing is painted.
  Measured once against Venice (`gpt-image-2-5-flare-edit`, a synthetic
  1536 × 1024 scene, an empty prompt over a painted ball): the ball's area went
  from the ball's yellow (mean 213, 193, 70) to the sky's blue (108, 164, 222),
  and nothing outside the painted area changed by more than one level of 255.
  The extra image costs Venice's per-image input price (USD 0.00966 for that
  model in its `/models` listing of 2026-10-17).
- **The answer.** Such an endpoint picks its own resolution, so the request
  is sent at the standard size (1536 × 1024, 1024 × 1536 or 1024 × 1024 by
  orientation) and the answer is resized to it when it has the same aspect —
  within 3%, the rounding a provider's fixed ratios introduce — and refused at
  any other. What of the answer is kept is unchanged — the painted area for a
  fill, the area the model changed for an edit without one, the whole frame
  for the whole-image generation. One request per edit, and none re-posted
  after the provider accepted it.
- **Where the key goes.** The path may not name another host (`//`, `://`,
  `@`, `..` and the like are refused), so the base URL alone decides where
  the key is sent; and the setting is trusted like the base URL — the central
  settings file and the environment may set it, a settings file in the working
  directory may not.
- **Sign in with ChatGPT.** The subscription bridge serves OpenAI's call, so
  under that option the choice is hidden and a saved template waits until the
  API option is chosen again.

## Compatibility

- **Settings.** One new key, `image_gen_endpoint`, absent by default; a
  settings file without it makes the OpenAI call as before. A v1.10.0 build
  that saves its Settings drops the key, since it does not know it.
- **Nothing else moves.** Recipes, stores, the reference library, the command
  line's commands and the browser page's routes are those of v1.10.0; the
  sidecars under `python/` are byte for byte v1.10.0's, and the weights stay
  pinned to the v1.6.0 `autoshade-raw-denoise-v2.pth`.

## Gates

Measured before the tag on the release code (`ebd314c`). The version bump
touches Cargo.toml, Cargo.lock, the documents, the site's cache keys and the
bug template's dropdown, and the CLI, contract and doc-test suites, clippy,
`check_docs`, `check_doc_drift`, the i18n audit and the font check are re-run
after it. The three-lane release battery (`scripts/release_battery.sh`, a
frozen snapshot worktree, the p36–p41 calibration corpus and the sidecar
weights in reach): library **1640 passed / 0 failed / 15 ignored** (868.93 s,
release profile, one process per module), CLI **25 / 0**, contract 2 + 2,
doc-tests 0, GUI **241 / 0 / 3**, calibration lane **1640 / 0 / 15**
(1127.79 s); `audit_i18n`, the font check (898 of 899 code points embedded,
one left to egui's bundle) and `check_doc_drift` (0 findings over 14
documents) exit 0 inside the battery. clippy 0 warnings on both feature sets
(`--all-targets -- -D warnings`). `check_docs.py --gates` on the transcript
with the XMP census root supplied: **32 PASS / 0 FAIL / 0 SKIP**.

By name against the v1.10.0 release battery: library 1648 → 1655, **+7 / −0**
— six in `image_endpoint` (the settings example parses; the painted area
reaches a provider with no mask parameter as a red-marked copy only when there
is one; placeholders expand once and lone painted-area entries drop; templates
that could misroute the key, and typos, are refused; replies are read by
pointer or as the body; the answer is resized only at the same aspect) and one
in `generative` (one JSON request with the template filled in, at the standard
size, the answer resized, a refusal reported, and the subscription bridge
still sent OpenAI's call). GUI 244, unchanged; CLI 25, unchanged.

The Python suites were not run: `python/`, `scripts/` and `tests/` are byte
for byte v1.10.0's (the ruling of 2026-10-13: run what changed). Final gate,
reference pair: not re-run. The reverse fit and the develop are untouched —
the batch changes the generative call, the settings and the documents — and a
recipe renders as in v1.10.0.

Mutations falsified the new invariants on the release code, each red and each
restored (SHA-256 equal): the template applied under the subscription bridge
turned `a_custom_endpoint_posts_the_template_and_resizes_the_answer` red; a
lone painted-area placeholder kept when nothing is painted, and the marking
tinted blue instead of red, each turned
`the_venice_example_shows_the_painted_area_only_when_there_is_one` red; an
`@` let through in the path turned
`templates_that_could_misroute_the_key_or_typo_are_refused` red; a 20% aspect
tolerance turned `the_answer_is_resized_at_the_same_aspect_and_refused_at_another`
red; and the setting trusted as a mere preference turned
`a_working_directory_settings_file_cannot_redirect_your_api_key` red. One
paid run against Venice is reported above; it is not part of the battery.
