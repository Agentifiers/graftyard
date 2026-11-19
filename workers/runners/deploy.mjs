// Builds `graft-runners` and deploys it to the runner account (design/DESIGN.md, Shape, `graft-runners`; Deployment
// modes, `actions state`). Not run yet: the runner account and every hosted run wait for the operator's approval of
// the cost estimate (tests/m4/results/m4-wp4.md, "Cost estimate"). Usage:
//   node workers/runners/deploy.mjs <runners.json> [--skip-build] [--config-only]
// `<runners.json>`: {"script", "graftyard_url", "app_id", "owners": {"<login>": <installation id>}, "account_vcpu",
// "app_key": "<Secrets Store id>/<secret name>", "max_instances": {"std3", "std4"}, "runner_box": false}.
// `runner_box` true makes the version start runners on `RunnerBox` (decisions/DECISIONS.md, `r23-runnerbox`). The
// legacy classes' `max_instances` bound only them: `RunnerBox`'s `durable_object` application has no instance cap,
// so its starts are bounded by the owner's cap, RUNNER_VCPU_STOP_PCT, and the account's vCPU. The webhook secret is set with
// `wrangler secret put WEBHOOK_SECRET`. `--config-only` writes the wrangler config and stops.
// Needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID of the runner account, never the install's.

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const [cfgPath, ...flags] = process.argv.slice(2);
if (!cfgPath) throw new Error("usage: node workers/runners/deploy.mjs <runners.json> [--skip-build] [--config-only]");
const c = JSON.parse(readFileSync(cfgPath, "utf8"));
const [storeId, secretName] = c.app_key.split("/");
const image = resolve(here, "../../containers/runner/Dockerfile");

const config = {
  name: c.script,
  main: "entry.mjs",
  compatibility_date: "2026-10-01",
  // The Sandbox SDK's bundle imports `node:path/posix`; its templates set this flag.
  compatibility_flags: ["nodejs_compat"],
  workers_dev: true,
  preview_urls: false,
  vars: {
    GRAFTYARD_URL: c.graftyard_url,
    HOSTED_RUNNERS_APP_ID: String(c.app_id),
    HOSTED_OWNERS: JSON.stringify(c.owners),
    ACCOUNT_VCPU: String(c.account_vcpu ?? 15000),
    ...(c.runner_box ? { RUNNER_BOX: "1" } : {}),
  },
  secrets_store_secrets: [{ binding: "HOSTED_RUNNERS_APP_KEY", store_id: storeId, secret_name: secretName }],
  durable_objects: {
    bindings: [
      { name: "SANDBOX_STD3", class_name: "SandboxStd3" },
      { name: "SANDBOX_STD4", class_name: "SandboxStd4" },
      { name: "RUNNER_BOX", class_name: "RunnerBox" },
    ],
  },
  containers: [
    { class_name: "SandboxStd3", image, instance_type: "standard-3", max_instances: c.max_instances?.std3 ?? 20 },
    { class_name: "SandboxStd4", image, instance_type: "standard-4", max_instances: c.max_instances?.std4 ?? 5 },
    // Each start picks the instance type and takes this version's `runner` image (`box.mjs`).
    { class_name: "RunnerBox", scheduling_policy: "durable_object", images: { runner: { dockerfile: image } } },
  ],
  migrations: [
    { tag: "v1", new_sqlite_classes: ["SandboxStd3", "SandboxStd4"] },
    { tag: "v2", new_sqlite_classes: ["RunnerBox"] },
  ],
  // SHARED's sweep, every 5 minutes [guess].
  triggers: { crons: ["*/5 * * * *"] },
  observability: { enabled: true },
};

const gen = join(here, ".gen");
mkdirSync(gen, { recursive: true });
const out = join(gen, `${c.script}.jsonc`);
// wrangler resolves `main` against the config's folder.
writeFileSync(out, JSON.stringify({ ...config, main: "../entry.mjs" }, null, 2));
console.log(`wrote ${out}`);
if (flags.includes("--config-only")) process.exit(0);

const run = (cmd, args, cwd) => execFileSync(cmd, args, { cwd, stdio: "inherit" });
if (!flags.includes("--skip-build")) {
  run("worker-build", ["--release"], here);
  run("npm", ["install", "--no-audit", "--no-fund"], here);
}
run("npx", ["wrangler", "deploy", "-c", out], here);
