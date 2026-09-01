# AutoShade static site

This directory is a build-free, self-contained Cloudflare Pages site. Its HTML, CSS, headers, and local images can be published as-is; no package install or asset compilation is required.

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

The script reads the master token from the git-ignored `.secret` file, mints a one-hour token scoped to Cloudflare Pages, runs `wrangler pages deploy site --project-name autoshade`, and deletes the temporary token afterwards. No token value is printed or written anywhere. The production alias is `autoshop-d7w.pages.dev` (the pages.dev subdomain is sticky across the project rename and kept as a legacy alias); each deployment also gets its own preview URL. After deploying, verify every published file byte-for-byte against `site/` before calling it live.

**Image URLs carry `?v=<release>`.** `_headers` gives `/images/*` a seven-day `max-age`, which is right for bytes that rarely change and wrong on the day they do: after the v1.2.0 deploy the apex served the previous Pillar 1 diagram from cache (`cf-cache-status: HIT`, `Age: 80753`) while the `pages.dev` alias, which is not behind that cache, already served the new one. The query string puts the release in the cache key, so a changed image is a new object rather than a week-old one. Bump it whenever `site/images/` changes. `scripts/purge_site_cache.js` exists for the same problem and does NOT work with the current master token: it mints a zone-scoped token that verifies active and can `GET /zones/<id>`, but `POST /zones/<id>/purge_cache` answers 401, i.e. the master token cannot delegate Cache Purge. Purging needs either a master token that carries that permission or one click in the dashboard (Caching -> Configuration -> Purge Everything).

The custom domain `autoshade.dev` (and `www.`) is attached to the `autoshade` Pages project (renamed in place from `autoshop`, deployments preserved): the zone lives in the same account and both names are proxied CNAME records pointing at `autoshop-d7w.pages.dev`, with certificates issued by Pages. Re-attaching after a project rebuild is done through the Pages project's **Custom domains** page or the Pages domains API.

Do not store deployment credentials in this directory or commit them to the repository.
