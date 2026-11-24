// Deploys `graft-agents-front` (`workers/agents-front`, `src/worker.rs`) to the agents account (design/AGENT-LAYER.md,
// Shape, "Controller"; build/BUILD-PLAN.md, AM3-WP1). Usage:
//   node workers/agents-front/deploy.mjs <name> --controller <graft-controller script> --anchor <install.toml>
//     --host <install host> --app-id <graftyard-agent App id> --key <store id>/<secret name> [--domain <host>]...
//     [--upload-only]
// - `CONTROLLER` is the `fetch()` service binding to `--controller`, and `CONTROLLER_SCRIPT` the name the version
//   override names.
// - `GRAFTYARD_INSTALL` and `GRAFTYARD_CUSTOMER_KEY` come from `--anchor`, as `graft-front` takes them, and
//   `GRAFTYARD_HOST` is `--host`, where the view is read (`GET /api/graftyard/v1/release/entries`): the `AgentsAnchor`.
// - `AGENT_APP_ID` is `--app-id`, and `AGENT_APP_KEY` the Secrets Store binding of `--key`, `graftyard-agent`'s PEM key,
//   which signs the App JWT of each read.
// - `VERSION` is the version metadata binding, the version `/_agents_front/status` reports.
// - The workers.dev route stays on, because a test install's webhook hostname may be a workers.dev name; each
//   `--domain` is a Workers Custom Domain of this script (`routes` with `custom_domain`; operator setup, outside any
//   request path).
// `FRONT_TOKEN` (the `/_agents_front/status` secret) is set separately with `wrangler secret put FRONT_TOKEN`, its value
// on stdin; no secret value passes through this script. `--upload-only` uploads a version without deploying it
// (`wrangler versions upload`), for a `ControllerRoll`, which the agents `graft-ops` deploys by percentage. Needs
// CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID of the agents account (the runner account on test installs). This is
// the ops path: no request path runs it.
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const [name, ...flags] = process.argv.slice(2);
const flag = (f) => { const i = flags.indexOf(f); return i >= 0 ? flags[i + 1] : undefined; };
const [storeId, secretName] = flag("--key")?.split("/") ?? [];
const [controller, anchorPath, host, appId] = ["--controller", "--anchor", "--host", "--app-id"].map(flag);
if (!name || name.startsWith("--") || !controller || !anchorPath || !host || !appId || !storeId || !secretName) {
  throw new Error("usage: node workers/agents-front/deploy.mjs <name> --controller <script> --anchor <install.toml> --host <install host> --app-id <id> --key <store id>/<secret name> [--domain <host>]... [--upload-only]");
}
if (!/^\d+$/.test(appId)) throw new Error("--app-id is the App's numeric id");
const domains = flags.flatMap((f, i) => (f === "--domain" ? [flags[i + 1]] : []));
const anchor = Object.fromEntries(readFileSync(anchorPath, "utf8").split("\n")
  .map((l) => l.match(/^(install|customer_key)\s*=\s*"([^"]*)"/)).filter(Boolean).map((m) => [m[1], m[2]]));
if (!anchor.install || !anchor.customer_key) throw new Error(`${anchorPath} has no install or customer_key`);

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
  services: [{ binding: "CONTROLLER", service: controller }],
  ...(domains.length ? { routes: domains.map((pattern) => ({ pattern, custom_domain: true })) } : {}),
  version_metadata: { binding: "VERSION" },
  secrets_store_secrets: [{ binding: "AGENT_APP_KEY", store_id: storeId, secret_name: secretName }],
  vars: {
    GRAFTYARD_INSTALL: anchor.install,
    GRAFTYARD_CUSTOMER_KEY: anchor.customer_key,
    GRAFTYARD_HOST: host,
    AGENT_APP_ID: appId,
    CONTROLLER_SCRIPT: controller,
  },
  observability: { enabled: true },
};
writeFileSync(join(gen, `${name}.jsonc`), JSON.stringify(cfg, null, 2));
const cmd = flags.includes("--upload-only") ? ["versions", "upload"] : ["deploy"];
execFileSync("npx", ["--prefix", resolve(here, "../../tests/m0"), "wrangler", ...cmd, "-c", `${name}.jsonc`],
  { cwd: gen, stdio: "inherit" });
