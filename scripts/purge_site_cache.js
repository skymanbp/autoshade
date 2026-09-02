"use strict";
// Purge specific URLs from Cloudflare's edge cache for the custom domain.
//
// Why this exists. `site/_headers` gives `/images/*` a seven-day
// `max-age=604800`, which is right for assets whose bytes rarely change — and
// wrong on the day they do. A deploy replaces the file at the origin, but the
// apex keeps serving the cached copy until the TTL runs out: after the v1.2.0
// deploy, `autoshade.dev` returned the pre-regeneration Pillar 1 diagram
// (`cf-cache-status: HIT`, `Age: 80753`) while `autoshop-d7w.pages.dev`, which
// is not in front of that cache, already served the new one. Six more days of
// a diagram whose caption says "three rulers" when the score has four terms.
//
// The personal token in `.secret` carries Cache Purge on the zone since
// 2026-09-02 (before that it could only mint, and a minted zone-scoped token
// still answered HTTP 401 on purge_cache — two mints, zone lookup then purge,
// were tried and both failed the same way). So the purge is one direct call
// with the personal token: look the zone up by name, purge, print the status.
// The token is read in-process and never printed or written. `deploy_site.js`
// calls `purgeEverything` after every successful deploy; the CLI form below is
// for purging a handful of URLs by hand.
//
// Usage: node scripts/purge_site_cache.js <url> [url ...]
//        node scripts/purge_site_cache.js --everything
const fs = require("fs");
const path = require("path");

const root = path.join(__dirname, "..");
const API = "https://api.cloudflare.com/client/v4";
const ZONE_NAME = "autoshade.dev";

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

async function zoneId(token) {
  const zones = await cf("GET", `/zones?name=${encodeURIComponent(ZONE_NAME)}`, token);
  if (zones.length !== 1) throw new Error(`expected one zone named ${ZONE_NAME}, got ${zones.length}`);
  return zones[0].id;
}

async function purge(token, body, label) {
  const zone = await zoneId(token);
  await cf("POST", `/zones/${zone}/purge_cache`, token, body);
  console.log(`[purge] ${label} purged on ${ZONE_NAME}`);
}

async function purgeEverything(token) {
  await purge(token, { purge_everything: true }, "everything");
}

async function main() {
  const args = process.argv.slice(2);
  if (args.length === 0) {
    console.error("usage: node scripts/purge_site_cache.js <url> [url ...] | --everything");
    process.exit(2);
  }
  const token = fs.readFileSync(path.join(root, ".secret"), "utf8").trim();
  if (args[0] === "--everything") {
    await purgeEverything(token);
  } else {
    await purge(token, { files: args }, `${args.length} url(s)`);
    for (const u of args) console.log(`         ${u}`);
  }
}

module.exports = { purgeEverything };

if (require.main === module) {
  main().catch((err) => {
    console.error(`[purge_site_cache] ${err.message}`);
    process.exit(1);
  });
}
