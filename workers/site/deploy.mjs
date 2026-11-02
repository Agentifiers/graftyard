// Builds the site script once and deploys it as every site of an install, each binding every site's member and
// registry classes with `script_name` (design/DESIGN.md, Shape, "Members reach each other through Durable Object
// bindings"). A class must exist before another script binds it, so a site that is new to the install is first deployed
// binding only its own classes. Usage:
//   node workers/site/deploy.mjs <sites.json> [--genesis <genesis.json>] [--skip-build] [--test-faults] [--vars <vars.json>]
//                                [--image <digest-pinned image>] [--anchor <install.toml>]
// `--test-faults` builds with the `test-faults` feature, for test installs only; `--vars` adds variables to every site
// (the 24-hour run's GRAFTYARD_FAULTS and GRAFTYARD_REQUEST_RETENTION_S).
// `--genesis` (test-faults installs only) names the system chains' records, which each site reads from
// GRAFTYARD_GENESIS in place of the `Release` view; a product install reads them from the genesis its `Release`
// group accepted, and needs `--anchor` (the trust anchor `graftyard init` wrote: GRAFTYARD_INSTALL and
// GRAFTYARD_CUSTOMER_KEY). Every site binds every site's bucket (`BUCKET_<n>`, created if missing) and hosts the
// git cache shard class `GitShard` on Cloudflare Containers, with `--image` (a digest-pinned image in the account's
// registry) or, without it, an image built from containers/git.
// Needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID. Every run creates a new version of each site script, which
// resets its members: outside a fresh install, that is a site day (AGENTS.md, invariants).

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const [sitesPath, ...flags] = process.argv.slice(2);
if (!sitesPath) throw new Error("usage: node workers/site/deploy.mjs <sites.json> [--genesis <genesis.json>] [--skip-build] [--test-faults] [--vars <vars.json>]");
const sites = JSON.parse(readFileSync(sitesPath, "utf8"));
const gi = flags.indexOf("--genesis");
const genesis = gi >= 0 ? JSON.parse(readFileSync(flags[gi + 1], "utf8")).placements : null;
const vi = flags.indexOf("--vars");
// Keys starting with `_` are comments.
const extraVars = Object.fromEntries(
  Object.entries(vi >= 0 ? JSON.parse(readFileSync(flags[vi + 1], "utf8")) : {}).filter(([k]) => !k.startsWith("_")),
);
const ii = flags.indexOf("--image");
const image = ii >= 0 ? flags[ii + 1] : null;
const ai = flags.indexOf("--anchor");
const anchor = ai >= 0 ? Object.fromEntries(
  readFileSync(flags[ai + 1], "utf8").split("\n").map((l) => l.match(/^(install|customer_key)\s*=\s*"([^"]*)"/)).filter(Boolean).map((m) => [m[1], m[2]]),
) : null;
if (genesis && !flags.includes("--test-faults")) throw new Error("--genesis is a test-faults shortcut; a product install reads the Release view");
const COMPAT_DATE = "2026-10-01";
// The sweeper's Cron Trigger [guess]: deletes wait 24 h (ORPHAN_AFTER), so a quarter-hour cadence only bounds how late
// a missing `Committed` marker or third-bucket copy is repaired.
const SWEEP_CRON = "*/15 * * * *";
const gen = join(here, ".gen");
mkdirSync(gen, { recursive: true });

if (!flags.includes("--skip-build")) {
  const features = flags.includes("--test-faults") ? ["--features", "test-faults"] : [];
  execFileSync("worker-build", ["--release", ...features], { cwd: here, stdio: "inherit" });
}

const wrangler = (args) =>
  execFileSync("npx", ["--prefix", resolve(here, "../../tests/m0"), "wrangler", ...args], { cwd: gen, stdio: "inherit" });

// A site binds only its own `GitShard` class: the edge reaches shards, members never do, and Cloudflare attaches the
// container config to the first binding of the class, which must be the script's own.
function bindings(self, targets) {
  return targets.flatMap((s) =>
    [["MEMBER", "Member"], ["REGISTRY", "Registry"], ["SHARD", "GitShard"]]
      .filter(([b]) => b !== "SHARD" || s.id === self.id)
      .map(([b, cls]) => ({
        name: `${b}_${s.id}`,
        class_name: cls,
        ...(s.id === self.id ? {} : { script_name: s.script }),
      })),
  );
}

function config(site, targets) {
  return {
    name: site.script,
    main: "../build/index.js",
    compatibility_date: COMPAT_DATE,
    workers_dev: false,
    preview_urls: false,
    vars: {
      SITE_ID: String(site.id),
      GRAFTYARD_SITES: JSON.stringify(sites),
      ...(genesis ? { GRAFTYARD_GENESIS: JSON.stringify(genesis) } : {}),
      ...(anchor ? { GRAFTYARD_INSTALL: anchor.install, GRAFTYARD_CUSTOMER_KEY: anchor.customer_key } : {}),
      ...Object.fromEntries(Object.entries(extraVars).map(([k, v]) => [k, typeof v === "string" ? v : JSON.stringify(v)])),
    },
    durable_objects: { bindings: bindings(site, targets) },
    r2_buckets: sites.map((s) => ({ binding: `BUCKET_${s.id}`, bucket_name: s.bucket })),
    containers: [{
      class_name: "GitShard",
      scheduling_policy: "durable_object",
      images: { git: image ? { image } : { dockerfile: resolve(here, "../../containers/git/Dockerfile") } },
    }],
    limits: { cpu_ms: 300000 },
    // The pack sweeper (workers/site/src/sweep.rs). Each site sweeps the networks it is home for.
    triggers: { crons: [SWEEP_CRON] },
    // v2 adds the git cache shard class to scripts that v1 created with members and registries only.
    migrations: [
      { tag: "v1", new_sqlite_classes: ["Member", "Registry"] },
      { tag: "v2", new_sqlite_classes: ["GitShard"] },
    ],
    observability: { enabled: true },
  };
}

// Every site's bucket, in its site's location.
const buckets = JSON.parse(
  execFileSync("curl", ["-sf", "-H", `Authorization: Bearer ${process.env.CLOUDFLARE_API_TOKEN}`,
    `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/r2/buckets?per_page=1000`]),
).result.buckets.map((b) => b.name);
for (const s of sites.filter((s) => !buckets.includes(s.bucket))) {
  wrangler(["r2", "bucket", "create", s.bucket, "--location", s.region]);
}

const listed = JSON.parse(
  execFileSync("curl", [
    "-sf",
    "-H", `Authorization: Bearer ${process.env.CLOUDFLARE_API_TOKEN}`,
    `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/workers/scripts`,
  ]),
).result.map((s) => s.id);

// Phase 1: every site gets its own classes (a new site binds only itself; an existing one keeps its member and
// registry bindings to the sites that exist).
for (const site of sites) {
  const targets = listed.includes(site.script) ? sites.filter((s) => s === site || listed.includes(s.script)) : [site];
  writeFileSync(join(gen, `${site.script}.jsonc`), JSON.stringify(config(site, targets), null, 2));
  wrangler(["deploy", "-c", `${site.script}.jsonc`]);
}
for (const site of sites) {
  writeFileSync(join(gen, `${site.script}.jsonc`), JSON.stringify(config(site, sites), null, 2));
  wrangler(["deploy", "-c", `${site.script}.jsonc`]);
}
