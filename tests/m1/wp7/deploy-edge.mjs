// Deploys the M1-WP7 test edges graft-m1-wp7-edge and graft-m1-wp7-edge-b from edge.ts. Each binds the Member and
// Registry classes of graft-m1-site-a/b/c with script_name and carries the test-only system chain records from
// genesis.json in its GENESIS variable, the same records deploy.mjs --genesis gives the sites.
//
//   node tests/m1/wp7/deploy-edge.mjs        # then: wrangler secret put TOKEN -c .gen/<name>.jsonc, once per edge
//
// Another install: SITES=<sites.json> GENESIS=<{"placements": ...}> EDGES=<name>[,<name>] (tests/m2/results/install-m2b.md).
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const gen = join(here, ".gen");
mkdirSync(gen, { recursive: true });
const genesis = JSON.parse(readFileSync(process.env.GENESIS ?? join(here, "genesis.json"), "utf8"));
const sites = JSON.parse(readFileSync(process.env.SITES ?? join(here, "../wp1/sites.json"), "utf8"));
for (const name of process.env.EDGES?.split(",") ?? ["graft-m1-wp7-edge", "graft-m1-wp7-edge-b"]) {
  const cfg = {
    name,
    main: "../edge.ts",
    compatibility_date: "2026-10-01",
    workers_dev: true,
    preview_urls: false,
    vars: { GENESIS: JSON.stringify({ placements: genesis.placements }), EDGE_NAME: name },
    durable_objects: { bindings: sites.flatMap((s) => [
      { name: `MEMBER_${s.id}`, class_name: "Member", script_name: s.script },
      { name: `REGISTRY_${s.id}`, class_name: "Registry", script_name: s.script },
    ]) },
    observability: { enabled: true },
  };
  writeFileSync(join(gen, `${name}.jsonc`), JSON.stringify(cfg, null, 2));
  execFileSync("npx", ["--prefix", resolve(here, "../../m0"), "wrangler", "deploy", "-c", `${name}.jsonc`], { cwd: gen, stdio: "inherit" });
}
