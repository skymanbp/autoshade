# Autoshop static site

This directory is a build-free, self-contained Cloudflare Pages site. Its HTML, CSS, headers, and local images can be published as-is; no package install or asset compilation is required.

## Local preview

From the repository root:

```text
python -m http.server --directory site 8000
```

Then open `http://localhost:8000/`. This command is only a local preview; the production host applies the rules in `_headers`.

## Cloudflare Pages deployment

Deployment is intentionally a separate supervisor step. From the repository root:

1. Provide `CLOUDFLARE_API_TOKEN` through the deployment environment.
2. Publish the directory:

   ```text
   wrangler pages deploy site --project-name autoshop
   ```

3. In the Cloudflare dashboard, open the `autoshop` Pages project, choose **Custom domains**, and add `skymanbp-autoshop.dev`. Follow the dashboard's ownership and DNS prompts.

Do not store deployment credentials in this directory or commit them to the repository.
