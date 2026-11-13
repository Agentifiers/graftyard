// Deploys the git edge (workers/edge) as graft-m1-edge against the site members of an install: one MEMBER_<n>,
// REGISTRY_<n>, and SHARD_<n> binding per site with script_name, and BUCKET_<n> per site. Usage:
//   node tests/m1/integ/deploy-edge.mjs <sites.json> <name> [--test-faults] [--genesis <genesis.json>]
//                                       [--anchor <install.toml>] [--no-shard] [--ops <graft-ops name>] [--var K=V ...]
//                                       [--web <web/dist/client>]
// `--genesis` (test-faults only) is the test install's stand-in for the Release view; `--anchor` sets the trust anchor
// a product edge accepts the Release group's genesis against. `--no-shard` leaves out SHARD_<n>, for site scripts
// with no `GitShard` class (git requests then fail; the release endpoint does not need a shard). `graftyard sites
// add` deploys the class, so an install it built needs no `--no-shard`.
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../..");
const [sitesPath, name, ...flags] = process.argv.slice(2);
if (!sitesPath || !name) throw new Error("usage: deploy-edge.mjs <sites.json> <name> [--test-faults] [--genesis g.json] [--anchor install.toml] [--no-shard] [--var K=V]");
const sites = JSON.parse(readFileSync(sitesPath, "utf8"));
const flag = (f) => { const i = flags.indexOf(f); return i >= 0 ? flags[i + 1] : null; };
const tf = flags.includes("--test-faults");
const noShard = flags.includes("--no-shard");
const genesis = flag("--genesis") ? JSON.parse(readFileSync(flag("--genesis"), "utf8")).placements : null;
if (genesis && !tf) throw new Error("--genesis is a test-faults shortcut");
const anchorFile = flag("--anchor");
const anchor = anchorFile ? Object.fromEntries(readFileSync(anchorFile, "utf8").split("\n")
  .map((l) => l.match(/^(install|customer_key)\s*=\s*"([^"]*)"/)).filter(Boolean).map((m) => [m[1], m[2]])) : null;
const extra = Object.fromEntries(flags.flatMap((f, i) => flags[i - 1] === "--var" ? [f.split(/=(.*)/s).slice(0, 2)] : []));

const edge = join(root, "workers/edge");
// A product build goes to its own directory, so it never replaces the test install's build under `build/`.
const out = tf ? join(edge, "build") : `/tmp/edge-prod-${name}`;
execFileSync("worker-build", ["--release", "--out-dir", out, ...(tf ? ["--features", "test-faults"] : [])], {
  cwd: edge, stdio: "inherit", env: { ...process.env, ...(tf ? {} : { CARGO_TARGET_DIR: "/tmp/edge-prod-target" }) },
});
const gen = join(here, ".gen");
mkdirSync(gen, { recursive: true });
const cfg = {
  name,
  main: join(out, "index.js"),
  compatibility_date: "2026-10-01",
  workers_dev: true,
  preview_urls: false,
  durable_objects: { bindings: sites.flatMap((s) => [["MEMBER", "Member"], ["REGISTRY", "Registry"], ["SHARD", "GitShard"]]
    .filter(([b]) => !(noShard && b === "SHARD")).map(([b, cls]) => ({ name: `${b}_${s.id}`, class_name: cls, script_name: s.script }))) },
  r2_buckets: sites.map((s) => ({ binding: `BUCKET_${s.id}`, bucket_name: s.bucket })),
  // `--web <dir>`: the web app's build (`npm run build` in web/, `VITE_MOCK_API=false`) as the `ASSETS` binding, with
  // the edge run first, so every path still reaches the edge (design/SHARED.md, "How the edge serves it").
  ...(flag("--web") ? { assets: { directory: resolve(flag("--web")), binding: "ASSETS", run_worker_first: true } } : {}),
  // `--ops <name>`: the `fetch()` binding to `graft-ops` that mints installation tokens (M3-WP4).
  ...(flag("--ops") ? { services: [{ binding: "OPS", service: flag("--ops") }] } : {}),
  vars: {
    PLAN: "pro", SSH_OPTION: "off", EDGE_CAP_BYTES: "80000000", FORMATS_ENABLED: "32.1", PART_BYTES: "5242880",
    HOST: `${name}.<workers-subdomain>.workers.dev`,
    GRAFTYARD_SITES: JSON.stringify(sites),
    ...(genesis ? { GRAFTYARD_GENESIS: JSON.stringify(genesis) } : {}),
    ...(anchor ? { GRAFTYARD_INSTALL: anchor.install, GRAFTYARD_CUSTOMER_KEY: anchor.customer_key } : {}),
    ...extra,
  },
  limits: { cpu_ms: 300000 },
  observability: { enabled: true },
};
writeFileSync(join(gen, `${name}.jsonc`), JSON.stringify(cfg, null, 2));
execFileSync("npx", ["--prefix", join(root, "tests/m0"), "wrangler", "deploy", "-c", `${name}.jsonc`], { cwd: gen, stdio: "inherit" });
