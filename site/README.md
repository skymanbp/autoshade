# Autoshop static site

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

To attach `skymanbp-autoshop.dev`, open the `autoshop` Pages project in the Cloudflare dashboard, choose **Custom domains**, and follow the ownership and DNS prompts (the zone must exist in the same account).

Do not store deployment credentials in this directory or commit them to the repository.
