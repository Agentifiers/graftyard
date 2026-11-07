// Deploys `graft-ops`'s minting Worker for a test install: no public route (`workers_dev` off), and the `MintWrapKey`
// secret bound from the account's Secrets Store as `MINT_WRAP_KEY`. The edge reaches it through its `OPS` service
// binding (`tests/m1/integ/deploy-edge.mjs --var` cannot add one; pass `--ops <name>` there). Usage:
//   node workers/ops/deploy.mjs <name> <secrets store id> <secret name>
// Needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID. The secret itself is written by `wrangler secrets-store secret
// create` with its value on stdin, never here.

import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const [name, storeId, secretName] = process.argv.slice(2);
if (!name || !storeId || !secretName) throw new Error("usage: deploy.mjs <name> <secrets store id> <secret name>");
const out = join(here, "build");
execFileSync("worker-build", ["--release", "--out-dir", out], { cwd: here, stdio: "inherit" });
const gen = join(here, ".gen");
mkdirSync(gen, { recursive: true });
const cfg = {
  name,
  main: join(out, "index.js"),
  compatibility_date: "2026-10-01",
  workers_dev: false,
  preview_urls: false,
  secrets_store_secrets: [{ binding: "MINT_WRAP_KEY", store_id: storeId, secret_name: secretName }],
  observability: { enabled: true },
};
writeFileSync(join(gen, `${name}.jsonc`), JSON.stringify(cfg, null, 2));
execFileSync("npx", ["--prefix", resolve(here, "../../tests/m0"), "wrangler", "deploy", "-c", `${name}.jsonc`], { cwd: gen, stdio: "inherit" });
