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

The script reads the master token from the git-ignored `.secret` file, mints a one-hour token scoped to Cloudflare Pages, runs `wrangler pages deploy site --project-name autoshop`, and deletes the temporary token afterwards. No token value is printed or written anywhere. The production alias is `autoshop-d7w.pages.dev`; each deployment also gets its own preview URL. After deploying, verify every published file byte-for-byte against `site/` before calling it live.

The custom domain `skymanbp-autoshop.dev` (and `www.`) is attached to the `autoshop` Pages project: the zone lives in the same account and both names are proxied CNAME records pointing at `autoshop-d7w.pages.dev`, with certificates issued by Pages. Re-attaching after a project rebuild is done through the Pages project's **Custom domains** page or the Pages domains API.

Do not store deployment credentials in this directory or commit them to the repository.
