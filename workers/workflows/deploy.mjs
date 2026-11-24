// Deploys the Workflows script (`entry.mjs`, M6-WP1) for an install. A new script with its own bindings: every site's
// `Member` and `Registry` classes by `script_name`, spares included, and the three Workflows it defines. Deploying it
// gives no site script a new version (`tests/m6/results/m6-wp1.md`, "C3"); `graft-ops` reaches it through its
// `WORKFLOWS` service binding (`workers/ops/deploy.mjs`, `"workflows"` in the executor file). Usage:
//   node workers/workflows/deploy.mjs <name> <sites.json>
// Needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID.

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const [name, sitesPath] = process.argv.slice(2);
if (!name || !sitesPath) throw new Error("usage: node workers/workflows/deploy.mjs <name> <sites.json>");
const sites = JSON.parse(readFileSync(sitesPath, "utf8"));
const gen = join(here, ".gen");
mkdirSync(gen, { recursive: true });
const wf = (cls, binding) => ({ name: `${name}-${cls.toLowerCase()}`, binding, class_name: cls });
const cfg = {
  name,
  main: resolve(here, "entry.mjs"),
  compatibility_date: "2026-10-01",
  workers_dev: false,
  preview_urls: false,
  durable_objects: {
    bindings: sites.flatMap((s) => [["MEMBER", "Member"], ["REGISTRY", "Registry"]]
      .map(([b, cls]) => ({ name: `${b}_${s.id}`, class_name: cls, script_name: s.script }))),
  },
  // A coordinator's steps: 64 shards per surviving site, a start per 100 groups, a refill pass per budget, and a poll
  // per POLL_MAX groups; a group's: at most ASKS_MAX asks (`move.mjs`).
  workflows: [wf("MoveCoordinator", "MOVE"), wf("MoveGroup", "GROUP"), wf("MoveExpire", "EXPIRE")],
  vars: { GRAFTYARD_SITES: JSON.stringify(sites) },
  observability: { enabled: true },
};
writeFileSync(join(gen, `${name}.jsonc`), JSON.stringify(cfg, null, 2));
execFileSync("npx", ["--prefix", resolve(here, "../../tests/m0"), "wrangler", "deploy", "-c", `${name}.jsonc`],
  { cwd: gen, stdio: "inherit" });
