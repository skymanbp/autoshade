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
// Same mint -> use -> delete shape as deploy_site.js, and the same rule: the
// master token in `.secret` is read in-process, and no token value is ever
// printed or written. This one is scoped to Cache Purge plus the Zone Read it
// needs to find the zone by name, so it cannot deploy and cannot read Pages.
//
// Usage: node scripts/purge_site_cache.js <url> [url ...]
//        node scripts/purge_site_cache.js --everything
const fs = require("fs");
const path = require("path");

const root = path.join(__dirname, "..");
const API = "https://api.cloudflare.com/client/v4";
const ACCOUNT = "ef6ce0a8b2c4ba8529b41aa6fd5b4f45";
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

const stamp = (d) => d.toISOString().replace(/\.\d{3}Z$/, "Z");

function pickGroup(groups, name) {
  const hit = groups.filter((g) => g.name === name);
  if (hit.length !== 1) throw new Error(`expected exactly one '${name}' group, got ${hit.length}`);
  return { id: hit[0].id, name };
}

// Two mints, not one, and the reason is the scope these groups actually carry.
// `Cache Purge` and `Zone Read` both report
// `scopes: ["com.cloudflare.api.account.zone"]` -- they are ZONE-level, so a
// policy whose resource is the ACCOUNT does not grant them: the first attempt
// authenticated fine, found the zone, and got HTTP 401 from purge_cache. The
// zone id is not known until it has been looked up, so the lookup gets its own
// short-lived token and the purge gets one scoped to exactly the zone the
// lookup returned. Nothing is hardcoded and neither token can do the other's job.
async function mintToken(master, label, groups, resources, minutes) {
  const now = Date.now();
  const expires = stamp(new Date(now + minutes * 60 * 1000));
  const made = await cf("POST", "/user/tokens", master, {
    name: `autoshade-${label}-${stamp(new Date(now)).replace(/[-:]/g, "")}`,
    policies: [{ effect: "allow", resources, permission_groups: groups }],
    not_before: stamp(new Date(now - 5 * 60 * 1000)),
    expires_on: expires,
  });
  console.log(`[mint] ${label} token id=${made.id} expires=${expires}`);
  return made;
}

async function main() {
  const args = process.argv.slice(2);
  if (args.length === 0) {
    console.error("usage: node scripts/purge_site_cache.js <url> [url ...] | --everything");
    process.exit(2);
  }
  const master = fs.readFileSync(path.join(root, ".secret"), "utf8").trim();
  const groups = await cf("GET", "/user/tokens/permission_groups", master);

  const finder = await mintToken(master, "zone-lookup", [pickGroup(groups, "Zone Read")],
    { [`com.cloudflare.api.account.${ACCOUNT}`]: "*" }, 10);
  let zone;
  try {
    const zones = await cf("GET", `/zones?name=${encodeURIComponent(ZONE_NAME)}`, finder.value);
    if (zones.length !== 1) throw new Error(`expected one zone named ${ZONE_NAME}, got ${zones.length}`);
    zone = zones[0].id;
  } finally {
    await cf("DELETE", `/user/tokens/${finder.id}`, master);
    console.log(`[cleanup] zone-lookup token ${finder.id} deleted`);
  }

  const purger = await mintToken(master, "cache-purge", [pickGroup(groups, "Cache Purge")],
    { [`com.cloudflare.api.account.zone.${zone}`]: "*" }, 10);
  try {
    const everything = args[0] === "--everything";
    await cf("POST", `/zones/${zone}/purge_cache`, purger.value,
      everything ? { purge_everything: true } : { files: args });
    console.log(`[purge] ${everything ? "everything" : `${args.length} url(s)`} purged`);
    if (!everything) for (const u of args) console.log(`         ${u}`);
  } finally {
    await cf("DELETE", `/user/tokens/${purger.id}`, master);
    console.log(`[cleanup] cache-purge token ${purger.id} deleted`);
  }
}

main().catch((err) => {
  console.error(`[purge_site_cache] ${err.message}`);
  process.exit(1);
});
