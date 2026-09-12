"use strict";
// Manual Cloudflare Pages deployment for site/ (there is no GitHub integration:
// pushing main does not publish). Same mint→deploy→delete shape as the
// CodeEraser site deploy, adapted to this project name.
//
// `.secret` at the repo root holds the user's personal token (kept there by
// the user's decision of 2026-09-02: the file is gitignored and read only
// in-process). It mints other tokens, and since 2026-09-02 it also carries
// Cache Purge on the zone. This script mints a one-hour token scoped to
// "Pages Write" on the account, hands it to wrangler through the environment,
// deletes it in `finally`, and then purges the edge cache of the custom domain
// with the personal token itself — `site/_headers` gives `/images/*` a seven-
// day `max-age`, so without the purge a deploy replaces the origin bytes and
// the apex keeps serving the old copy until the TTL runs out (seen after the
// v1.2.0 deploy: `cf-cache-status: HIT`, `Age: 80753`). No token value is ever
// printed or written.
const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");
const { purgeEverything } = require("./purge_site_cache.js");

const root = path.join(__dirname, "..");
const API = "https://api.cloudflare.com/client/v4";
// Account id is not a credential (it is in every dashboard URL); the project is
// created once with `POST /accounts/{id}/pages/projects` and then reused.
const ACCOUNT = "ef6ce0a8b2c4ba8529b41aa6fd5b4f45";
const PROJECT = "autoshade";

async function cf(method, route, bearer, body) {
  const res = await fetch(API + route, {
    method,
    headers: { Authorization: `Bearer ${bearer}`, "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const payload = await res.json();
  if (!res.ok || !payload.success) {
    throw new Error(`${method} ${route} -> HTTP ${res.status}: ${JSON.stringify(payload.errors)}`);
  }
  return payload.result;
}

const stamp = (d) => d.toISOString().replace(/\.\d{3}Z$/, "Z");

async function mint(master) {
  const groups = await cf("GET", "/user/tokens/permission_groups", master);
  const pagesWrite = groups.filter((g) => g.name === "Pages Write");
  if (pagesWrite.length !== 1) {
    throw new Error(`expected exactly one 'Pages Write' group, got ${pagesWrite.length}`);
  }
  const now = Date.now();
  const expires = stamp(new Date(now + 60 * 60 * 1000));
  const made = await cf("POST", "/user/tokens", master, {
    name: `autoshade-site-deploy-${stamp(new Date(now)).replace(/[-:]/g, "")}`,
    policies: [{
      effect: "allow",
      resources: { [`com.cloudflare.api.account.${ACCOUNT}`]: "*" },
      permission_groups: [{ id: pagesWrite[0].id, name: "Pages Write" }],
    }],
    not_before: stamp(new Date(now - 5 * 60 * 1000)),
    expires_on: expires,
  });
  console.log(`[mint] temp token id=${made.id} expires=${expires}`);
  return made;
}

// wrangler uploads every file under the directory it is given and reads no
// ignore file: a browsing tool's request log under the git-ignored
// site/.gstack/ went live with the 2026-09-01 deploy and stayed there until
// 2026-09-12. The site is what the repository tracks under site/, so exactly
// that list is copied into a fresh temporary directory and wrangler is
// pointed at the copy; `--stage-only` prints the list and stops there.
function stage() {
  const list = spawnSync("git", ["ls-files", "-z", "--", "site"], { cwd: root, encoding: "buffer" });
  if (list.status !== 0) throw new Error(`git ls-files exit=${list.status}`);
  const files = list.stdout.toString("utf8").split("\0").filter(Boolean);
  if (files.length === 0) throw new Error("git ls-files found nothing under site/");
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "autoshade-site-"));
  for (const rel of files) {
    const dest = path.join(dir, path.relative("site", rel));
    fs.mkdirSync(path.dirname(dest), { recursive: true });
    fs.copyFileSync(path.join(root, rel), dest);
  }
  console.log(`[stage] ${files.length} tracked files under site/ copied to ${dir}`);
  return { dir, files };
}

function deploy(tempValue, dir) {
  const args = ["wrangler", "pages", "deploy", dir, "--project-name", PROJECT, "--branch", "main", "--commit-dirty=true"];
  console.log(`[deploy] $ npx ${args.join(" ")}`);
  const run = spawnSync("npx", args, {
    cwd: root,
    shell: process.platform === "win32",
    stdio: "inherit",
    env: { ...process.env, CLOUDFLARE_API_TOKEN: tempValue, CLOUDFLARE_ACCOUNT_ID: ACCOUNT },
  });
  const status = run.status === null ? 1 : run.status;
  console.log(`[deploy] wrangler exit=${status}`);
  return status;
}

async function main() {
  const staged = stage();
  if (process.argv.includes("--stage-only")) {
    for (const f of staged.files) console.log(`  ${f}`);
    fs.rmSync(staged.dir, { recursive: true, force: true });
    return;
  }
  const master = fs.readFileSync(path.join(root, ".secret"), "utf8").trim();
  const temp = await mint(master);
  let status = 1;
  try {
    status = deploy(temp.value, staged.dir);
  } finally {
    await cf("DELETE", `/user/tokens/${temp.id}`, master);
    console.log(`[cleanup] temp token ${temp.id} deleted`);
    fs.rmSync(staged.dir, { recursive: true, force: true });
  }
  // A failed deploy leaves the old files at the origin, and purging then would
  // only cost cache hits, so the purge is conditional on wrangler's exit code.
  if (status === 0) await purgeEverything(master);
  process.exit(status);
}

main().catch((err) => {
  console.error(`[deploy_site] ${err.message}`);
  process.exit(1);
});
