// Deploys `graft-controller` (`workers/controller`, `src/worker.rs` and `src/taskbox.rs`) to the agents account
// (design/AGENT-LAYER.md, Shape, "Controller"; build/BUILD-PLAN.md, AM3-WP1). Usage:
//   node workers/controller/deploy.mjs <name> --secret <store id>/<secret name> [--bad] [--upload-only]
//     [--taskbox <taskbox.json>]
// - `WEBHOOK_SECRET` is the Secrets Store binding of `--secret`, the `graftyard-agent` webhook secret.
// - `VERSION` is the version metadata binding, the version report `graft-agents-front` checks against its
//   `ControllerPointer` pin (`x-graftyard-controller-version`), and the version a `TaskBox` runs.
// - No workers.dev route: only `graft-agents-front` reaches it, over its `fetch()` service binding.
// - `--taskbox` adds the `TaskBox` Durable Object (DECISIONS "`controller-taskbox`"): the class, its one
//   `durable_object`-policy container application, and from `<taskbox.json>` ({"agent_app_id", "workspace_app_id",
//   "store", "image", "agents_front"}) the `sandbox` image (a registry image, or a Dockerfile path relative to the
//   repository root, which wrangler builds), the two App ids, the Secrets Store bindings
//   `AGENT_APP_KEY`, `WORKSPACE_APP_KEY`, and `MODEL_KEY` in `store`, and `AGENTS_FRONT`, the `fetch()` binding to
//   `graft-agents-front` with `props` `{"pointer": true}`. `agents_front` is optional: the two scripts bind each other,
//   so the first deploy of an install omits it, and the one after `graft-agents-front` exists adds it.
// `STATUS_TOKEN` (the `/_controller/status` secret) is set separately with `wrangler secret put STATUS_TOKEN`, its
// value on stdin; no secret value passes through this script. `--bad` builds with the `test-bad` feature, LF-7's bad
// N+1, for test installs only. `--upload-only` uploads a version without deploying it (`wrangler versions upload`), for
// a `ControllerRoll`, which the agents `graft-ops` deploys by percentage. Needs CLOUDFLARE_API_TOKEN and
// CLOUDFLARE_ACCOUNT_ID of the agents account (the runner account on test installs). This is the ops path: no request
// path runs it.
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const [name, ...flags] = process.argv.slice(2);
const flag = (f) => { const i = flags.indexOf(f); return i >= 0 ? flags[i + 1] : undefined; };
const [storeId, secretName] = flag("--secret")?.split("/") ?? [];
if (!name || name.startsWith("--") || !storeId || !secretName) {
  throw new Error("usage: node workers/controller/deploy.mjs <name> --secret <store id>/<secret name> [--bad] [--upload-only] [--taskbox <taskbox.json>]");
}

const out = join(here, "build");
const features = flags.includes("--bad") ? ["--features", "test-bad"] : [];
execFileSync("worker-build", ["--release", "--out-dir", out, ...features], { cwd: here, stdio: "inherit" });
const gen = join(here, ".gen");
mkdirSync(gen, { recursive: true });
const tbPath = flag("--taskbox");
const tb = tbPath ? JSON.parse(readFileSync(tbPath, "utf8")) : null;
if (tb && (!/^\d+$/.test(String(tb.agent_app_id)) || !/^\d+$/.test(String(tb.workspace_app_id)) || !tb.store || !tb.image)) {
  throw new Error(`${tbPath}: needs agent_app_id, workspace_app_id, store, and image`);
}
const sandboxImage = tb && (/Dockerfile$/.test(tb.image) ? { dockerfile: resolve(here, "../..", tb.image) } : { image: tb.image });
const cfg = {
  name,
  // With the `TaskBox`, `entry.mjs` re-exports the Rust build and adds `Outbound`, the Sandbox's router.
  main: tb ? join(here, "entry.mjs") : join(out, "index.js"),
  compatibility_date: "2026-10-01",
  workers_dev: false,
  preview_urls: false,
  version_metadata: { binding: "VERSION" },
  secrets_store_secrets: [
    { binding: "WEBHOOK_SECRET", store_id: storeId, secret_name: secretName },
    ...(tb ? ["AGENT_APP_KEY", "WORKSPACE_APP_KEY", "MODEL_KEY"].map((n) => ({ binding: n, store_id: tb.store, secret_name: n })) : []),
  ],
  ...(tb ? {
    vars: { AGENT_APP_ID: String(tb.agent_app_id), WORKSPACE_APP_ID: String(tb.workspace_app_id) },
    ...(tb.agents_front ? { services: [{ binding: "AGENTS_FRONT", service: tb.agents_front, props: { pointer: true } }] } : {}),
    durable_objects: { bindings: [{ name: "TASKBOX", class_name: "TaskBox" }] },
    containers: [{ class_name: "TaskBox", scheduling_policy: "durable_object",
      images: { sandbox: sandboxImage } }],
    migrations: [{ tag: "v1", new_sqlite_classes: ["TaskBox"] }],
  } : {}),
  observability: { enabled: true },
};
writeFileSync(join(gen, `${name}.jsonc`), JSON.stringify(cfg, null, 2));
const cmd = flags.includes("--upload-only") ? ["versions", "upload"] : ["deploy"];
execFileSync("npx", ["--prefix", resolve(here, "../../tests/m0"), "wrangler", ...cmd, "-c", `${name}.jsonc`],
  { cwd: gen, stdio: "inherit" });
