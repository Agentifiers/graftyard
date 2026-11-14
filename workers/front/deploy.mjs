// Deploys `graft-front` (workers/front) for an install: the `EDGE` `fetch()` service binding to the edge script, the
// `VERSION` version metadata binding, one MEMBER_<n> and REGISTRY_<n> binding per site (its cached `Release` view),
// and the install's trust anchor. `FRONT_TOKEN` (the `/_front/status` secret) is written separately with
// `wrangler secret put`, its value on stdin. Usage:
//   node workers/front/deploy.mjs <sites.json> <name> --edge <edge script> --anchor <install.toml> [--upload-only]
// `--upload-only` uploads a version without deploying it (`wrangler versions upload`), for a `FrontRoll`, which the ops
// path deploys by percentage. Needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID. This is the ops path: no request
// path runs it.
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const [sitesPath, name, ...flags] = process.argv.slice(2);
const flag = (f) => { const i = flags.indexOf(f); return i >= 0 ? flags[i + 1] : null; };
if (!sitesPath || !name || !flag("--edge") || !flag("--anchor")) {
  throw new Error("usage: deploy.mjs <sites.json> <name> --edge <edge script> --anchor <install.toml> [--upload-only]");
}
const sites = JSON.parse(readFileSync(sitesPath, "utf8"));
const anchor = Object.fromEntries(readFileSync(flag("--anchor"), "utf8").split("\n")
  .map((l) => l.match(/^(install|customer_key)\s*=\s*"([^"]*)"/)).filter(Boolean).map((m) => [m[1], m[2]]));
const out = join(here, "build");
execFileSync("worker-build", ["--release", "--out-dir", out], { cwd: here, stdio: "inherit" });
const gen = join(here, ".gen");
mkdirSync(gen, { recursive: true });
const cfg = {
  name,
  main: join(out, "index.js"),
  compatibility_date: "2026-10-01",
  workers_dev: true,
  preview_urls: false,
  services: [{ binding: "EDGE", service: flag("--edge") }],
  version_metadata: { binding: "VERSION" },
  durable_objects: {
    bindings: sites.flatMap((s) => [["MEMBER", "Member"], ["REGISTRY", "Registry"]]
      .map(([b, cls]) => ({ name: `${b}_${s.id}`, class_name: cls, script_name: s.script }))),
  },
  vars: {
    GRAFTYARD_SITES: JSON.stringify(sites),
    GRAFTYARD_INSTALL: anchor.install,
    GRAFTYARD_CUSTOMER_KEY: anchor.customer_key,
    EDGE_SCRIPT: flag("--edge"),
  },
  observability: { enabled: true },
};
writeFileSync(join(gen, `${name}.jsonc`), JSON.stringify(cfg, null, 2));
const cmd = flags.includes("--upload-only") ? ["versions", "upload"] : ["deploy"];
execFileSync("npx", ["--prefix", join(root, "tests/m0"), "wrangler", ...cmd, "-c", `${name}.jsonc`], { cwd: gen, stdio: "inherit" });
