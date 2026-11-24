// Deploys `graft-controller` (`workers/controller`, `src/worker.rs`) to the agents account (design/AGENT-LAYER.md,
// Shape, "Controller"; build/BUILD-PLAN.md, AM3-WP1). Usage:
//   node workers/controller/deploy.mjs <name> --secret <store id>/<secret name> [--bad] [--upload-only]
// - `WEBHOOK_SECRET` is the Secrets Store binding of `--secret`, the `graftyard-agent` webhook secret.
// - `VERSION` is the version metadata binding, the version report `graft-agents-front` checks against its
//   `ControllerPointer` pin (`x-graftyard-controller-version`).
// - No workers.dev route: only `graft-agents-front` reaches it, over its `fetch()` service binding.
// `STATUS_TOKEN` (the `/_controller/status` secret) is set separately with `wrangler secret put STATUS_TOKEN`, its
// value on stdin; no secret value passes through this script. `--bad` builds with the `test-bad` feature, LF-7's bad
// N+1, for test installs only. `--upload-only` uploads a version without deploying it (`wrangler versions upload`), for
// a `ControllerRoll`, which the agents `graft-ops` deploys by percentage. Needs CLOUDFLARE_API_TOKEN and
// CLOUDFLARE_ACCOUNT_ID of the agents account (the runner account on test installs). This is the ops path: no request
// path runs it.
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const [name, ...flags] = process.argv.slice(2);
const flag = (f) => { const i = flags.indexOf(f); return i >= 0 ? flags[i + 1] : undefined; };
const [storeId, secretName] = flag("--secret")?.split("/") ?? [];
if (!name || name.startsWith("--") || !storeId || !secretName) {
  throw new Error("usage: node workers/controller/deploy.mjs <name> --secret <store id>/<secret name> [--bad] [--upload-only]");
}

const out = join(here, "build");
const features = flags.includes("--bad") ? ["--features", "test-bad"] : [];
execFileSync("worker-build", ["--release", "--out-dir", out, ...features], { cwd: here, stdio: "inherit" });
const gen = join(here, ".gen");
mkdirSync(gen, { recursive: true });
const cfg = {
  name,
  main: join(out, "index.js"),
  compatibility_date: "2026-10-01",
  workers_dev: false,
  preview_urls: false,
  version_metadata: { binding: "VERSION" },
  secrets_store_secrets: [{ binding: "WEBHOOK_SECRET", store_id: storeId, secret_name: secretName }],
  observability: { enabled: true },
};
writeFileSync(join(gen, `${name}.jsonc`), JSON.stringify(cfg, null, 2));
const cmd = flags.includes("--upload-only") ? ["versions", "upload"] : ["deploy"];
execFileSync("npx", ["--prefix", resolve(here, "../../tests/m0"), "wrangler", ...cmd, "-c", `${name}.jsonc`],
  { cwd: gen, stdio: "inherit" });
