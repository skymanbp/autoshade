# AutoShade static site

This directory is a build-free, self-contained Cloudflare Pages site. Its HTML, CSS, headers, local images and fonts can be published as-is; no package install or asset compilation is required.

The four diagrams are **inline SVG**, spliced into `index.html` and
`architecture.html` between `<!-- diagram:NAME -->` markers by
`scripts/pillar_diagrams.py` and `scripts/architecture_diagram.py`; run those
two after any change to a diagram and commit the pages they rewrite. Inline is
what lets the page's own light/dark tokens colour the drawing through the
`--dg-*` custom properties in `styles.css`, and it takes the diagrams out of
the `/images/*` cache described below — an HTML change is served immediately,
so the `?v=` key has to cover the photographs, `styles.css` and `diagram.js`
(see the caching section for why the last two). `diagram.js` is the one script this site loads: it gives every diagram wheel
zoom, drag pan, pinch, buttons and keyboard control, and `_headers` allows it
with `script-src 'self'` (it was `'none'`, which blocks an inline block
outright and would have left the controls hidden on the live site while every
local preview looked correct).

The page sans face lives in `fonts/Inter-autoshade.woff2`: the supplied 96,368-byte
Inter 4.001 variable subset, with optical size 14-32 and weight 100-900.
`fonts/OFL-Inter.txt` is its SIL Open Font License 1.1. All three pages preload
the same file with `crossorigin`; `styles.css` uses `font-display: swap`, a real
system fallback and automatic optical sizing. The monospace stack stays as it
was. Diagram labels keep their explicit system/Segoe UI stack because the
generators lay them out against measured metrics for that face.

## Local preview

From the repository root:

```text
python -m http.server --directory site 8000
```

Then open `http://localhost:8000/`. This command is only a local preview; the production host applies the rules in `_headers`.

## Cloudflare Pages deployment

Deployment is a manual step (there is no GitHub integration; pushing `main` does not publish). From the repository root:

```text
node scripts/deploy_site.js
```

The script copies the files git tracks under `site/` into a fresh temporary directory (wrangler uploads everything under the directory it is given and reads no ignore file: a browsing tool's request log under the git-ignored `site/.gstack/` went live with the 2026-09-01 deploy and stayed there until 2026-09-12), reads the master token from the git-ignored `.secret` file, mints a one-hour token scoped to Cloudflare Pages, runs `wrangler pages deploy <that copy> --project-name autoshade`, and deletes the temporary token and the copy afterwards; `node scripts/deploy_site.js --stage-only` prints the staged list and stops before any token is read. No token value is printed or written anywhere. The production alias is `autoshop-d7w.pages.dev` (the pages.dev subdomain is sticky across the project rename and kept as a legacy alias); each deployment also gets its own preview URL. After deploying, verify every published file byte-for-byte against `site/` before calling it live.

**Image URLs carry `?v=<release>`.** `_headers` gives `/images/*` a seven-day `max-age`, which is right for bytes that rarely change and wrong on the day they do: after the v1.2.0 deploy the apex served the previous Pillar 1 diagram from cache (`cf-cache-status: HIT`, `Age: 80753`) while the `pages.dev` alias, which is not behind that cache, already served the new one. The query string puts the release in the cache key, so a changed image is a new object rather than a week-old one. Bump it whenever `site/images/` changes. `scripts/purge_site_cache.js` exists for the same problem and does NOT work with the current master token: it mints a zone-scoped token that verifies active and can `GET /zones/<id>`, but `POST /zones/<id>/purge_cache` answers 401, i.e. the master token cannot delegate Cache Purge. Purging needs either a master token that carries that permission or one click in the dashboard (Caching -> Configuration -> Purge Everything).

**`styles.css` and `diagram.js` carry the key too.** The purge above empties the edge, not a visitor's browser, and on the custom domain the zone's default 4-hour Browser Cache TTL rewrites the Pages origin's `Cache-Control` on those two files to `max-age=14400` (the `pages.dev` alias, which is not behind the zone, serves the origin's `max-age=0, must-revalidate`). HTML is not edge-cached and keeps its `max-age=0`, so the 2026-09-06 hero deploy reached a browser that had loaded the page earlier that afternoon as the new `index.html` styled by the old `styles.css`: the owner line at full h1 size, the tagline as body text, the lede back in the retired right column. A `_headers` rule of `Cache-Control: no-cache` for the two files was tried the same day and came back from the apex as `max-age=14400` as well, so the header cannot fix this from inside the repository; the query-string key can, because a changed key is a URL the browser has never seen. Bump the key on `styles.css` (currently `?v=1.3.0`) or `diagram.js` (`?v=1.3.0`) whenever that file changes, wherever it is loaded: the stylesheet in all three HTML files, the script in `index.html` and `architecture.html`. The zone-level alternative is the dashboard's Browser Cache TTL set to "Respect Existing Headers", which would make the origin's `max-age=0` reach browsers and retire the key for these two files; that is a zone setting, not a repository change.

**Font filenames are their cache keys.** `_headers` gives `/fonts/*` a one-year `max-age` with `immutable`. The bundled subset is finished; if its bytes ever change, give it a new filename and update the font-face URL and all three preloads together. A long cache is safe because a changed font then has a new URL. The licence ships alongside it.

The custom domain `autoshade.dev` (and `www.`) is attached to the `autoshade` Pages project (renamed in place from `autoshop`, deployments preserved): the zone lives in the same account and both names are proxied CNAME records pointing at `autoshop-d7w.pages.dev`, with certificates issued by Pages. Re-attaching after a project rebuild is done through the Pages project's **Custom domains** page or the Pages domains API.

Do not store deployment credentials in this directory or commit them to the repository.
