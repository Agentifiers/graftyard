// Deploys `graft-ops`'s minting Worker for a test install: no public route (`workers_dev` off), and the `MintWrapKey`
// secret bound from the account's Secrets Store as `MINT_WRAP_KEY`. The edge reaches it through its `OPS` service
// binding (`tests/m1/integ/deploy-edge.mjs --var` cannot add one; pass `--ops <name>` there). Usage:
//   node workers/ops/deploy.mjs <name> <secrets store id> <secret name> [<wrap key slot A secret name>
//     [<slot B secret name> <current slot>]]
// The fourth argument binds `SecretsWrapKey`'s slot A as `SECRETS_WRAP_KEY_A` (current slot 0), for `/wrap` and
// `/unwrap` (M4-WP1). The fifth binds slot B as `SECRETS_WRAP_KEY_B` and the sixth names the current slot, for the
// wrapping key's rotation (M4-WP3, BF-59): deploy with both slots and B current, rewrap, then deploy with B alone.
// `--executor <executor.json>` also binds the executor of signed intents (M5-WP3, `src/exec.rs`) and its 5-minute
// Cron Trigger. The file holds `{"sites": <sites.json path>, "anchor": <install.toml path>, "scripts": {"site/0":
// "<name>", ..., "edge": "<name>"}, "deploy_token": <secret name>, "executor_key": <secret name>, "drain_key": <secret
// name>, "releases": <release bucket>, "gate": "pending", "step_dwell_ms": n, "first_day_ms": n}`; the last three are
// optional, and the dwells are for test installs only.
// Needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID. The secret itself is written by `wrangler secrets-store secret
// create` with its value on stdin, never here.

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const ex = args.indexOf("--executor");
const executor = ex >= 0 ? JSON.parse(readFileSync(args.splice(ex, 2)[1], "utf8")) : null;
const [name, storeId, secretName, wrapA, wrapB, current] = args;
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
  secrets_store_secrets: [
    { binding: "MINT_WRAP_KEY", store_id: storeId, secret_name: secretName },
    ...(wrapA && wrapA !== "-" ? [{ binding: "SECRETS_WRAP_KEY_A", store_id: storeId, secret_name: wrapA }] : []),
    ...(wrapB ? [{ binding: "SECRETS_WRAP_KEY_B", store_id: storeId, secret_name: wrapB }] : []),
  ],
  ...(wrapA ? { vars: { SECRETS_WRAP_KEY_CURRENT: current ?? "0" } } : {}),
  observability: { enabled: true },
};
if (executor) {
  const sites = JSON.parse(readFileSync(executor.sites, "utf8"));
  const anchor = Object.fromEntries(readFileSync(executor.anchor, "utf8").split("\n")
    .map((l) => l.match(/^(install|customer_key)\s*=\s*"([^"]*)"/)).filter(Boolean).map((m) => [m[1], m[2]]));
  cfg.secrets_store_secrets.push(
    { binding: "DEPLOY_TOKEN", store_id: storeId, secret_name: executor.deploy_token },
    { binding: "EXECUTOR_KEY", store_id: storeId, secret_name: executor.executor_key },
    ...(executor.drain_key ? [{ binding: "DRAIN_KEY", store_id: storeId, secret_name: executor.drain_key }] : []),
  );
  cfg.durable_objects = {
    bindings: sites.flatMap((s) => [["MEMBER", "Member"], ["REGISTRY", "Registry"]]
      .map(([b, cls]) => ({ name: `${b}_${s.id}`, class_name: cls, script_name: s.script }))),
  };
  if (executor.releases) cfg.r2_buckets = [{ binding: "RELEASES", bucket_name: executor.releases }];
  cfg.vars = {
    ...(cfg.vars ?? {}),
    GRAFTYARD_SITES: JSON.stringify(sites),
    GRAFTYARD_INSTALL: anchor.install,
    GRAFTYARD_CUSTOMER_KEY: anchor.customer_key,
    CF_ACCOUNT: process.env.CLOUDFLARE_ACCOUNT_ID,
    OPS_SCRIPTS: JSON.stringify(executor.scripts),
    // M5-WP4: unset reads the real gate; a test install may force `pass`, `pending`, or `breach:<why>`.
    ...(executor.gate ? { GATE: executor.gate } : {}),
    ...(executor.step_dwell_ms != null ? { STEP_DWELL_MS: String(executor.step_dwell_ms) } : {}),
    ...(executor.first_day_ms != null ? { FIRST_DAY_MS: String(executor.first_day_ms) } : {}),
  };
  cfg.triggers = { crons: ["*/5 * * * *"] };
}
writeFileSync(join(gen, `${name}.jsonc`), JSON.stringify(cfg, null, 2));
execFileSync("npx", ["--prefix", resolve(here, "../../tests/m0"), "wrangler", "deploy", "-c", `${name}.jsonc`], { cwd: gen, stdio: "inherit" });
