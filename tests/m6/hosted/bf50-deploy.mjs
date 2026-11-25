// Deploys `bf50-edge.ts` as `<name>` for an install's sites.json: every site's `Member` and `Registry` by
// `script_name`, spares included. A scratch script, deleted after the row; it creates no site version. Usage:
//   node tests/m6/hosted/bf50-deploy.mjs <sites.json> <name>      then put TOKEN once with wrangler secret put
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const [sitesPath, name] = process.argv.slice(2);
if (!sitesPath || !name) throw new Error("usage: bf50-deploy.mjs <sites.json> <name>");
const sites = JSON.parse(readFileSync(sitesPath, "utf8"));
const gen = join(here, ".gen");
mkdirSync(gen, { recursive: true });
const cfg = {
  name,
  main: resolve(here, "bf50-edge.ts"),
  compatibility_date: "2026-10-01",
  workers_dev: true,
  preview_urls: false,
  vars: { SITES: JSON.stringify(sites.map(({ id, region, spare, script }) => ({ id, region, spare, script }))) },
  durable_objects: {
    bindings: sites.flatMap((s) => [["MEMBER", "Member"], ["REGISTRY", "Registry"]]
      .map(([b, cls]) => ({ name: `${b}_${s.id}`, class_name: cls, script_name: s.script }))),
  },
  observability: { enabled: true },
};
writeFileSync(join(gen, `${name}.jsonc`), JSON.stringify(cfg, null, 2));
execFileSync("npx", ["--prefix", resolve(here, "../../m0"), "wrangler", "deploy", "-c", `${name}.jsonc`], { cwd: gen, stdio: "inherit" });
