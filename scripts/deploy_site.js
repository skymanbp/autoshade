"use strict";
// Manual Cloudflare Pages deployment for site/ (there is no GitHub integration:
// pushing main does not publish). Same mint→deploy→delete shape as the
// CodeEraser site deploy, adapted to this project name.
//
// `.secret` at the repo root holds the user's master token, which can only
// mint other tokens (it has no Pages permission, and listing /accounts with it
// is expectedly empty). This script mints a one-hour token scoped to
// "Pages Write" on the account, hands it to wrangler through the environment,
// and deletes it in `finally`. No token value is ever printed or written.
const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const root = path.join(__dirname, "..");
const API = "https://api.cloudflare.com/client/v4";
// Account id is not a credential (it is in every dashboard URL); the project is
// created once with `POST /accounts/{id}/pages/projects` and then reused.
const ACCOUNT = "ef6ce0a8b2c4ba8529b41aa6fd5b4f45";
const PROJECT = "autoshop";

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

function deploy(tempValue) {
  const args = ["wrangler", "pages", "deploy", "site", "--project-name", PROJECT, "--branch", "main", "--commit-dirty=true"];
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
  const master = fs.readFileSync(path.join(root, ".secret"), "utf8").trim();
  const temp = await mint(master);
  let status = 1;
  try {
    status = deploy(temp.value);
  } finally {
    await cf("DELETE", `/user/tokens/${temp.id}`, master);
    console.log(`[cleanup] temp token ${temp.id} deleted`);
  }
  process.exit(status);
}

main().catch((err) => {
  console.error(`[deploy_site] ${err.message}`);
  process.exit(1);
});
