// Deploys `graft-mcp` (`workers/mcp`, `src/worker.rs`) for a test install, from what `graftyard apps init` wrote:
//   node workers/mcp/deploy.mjs <name> <apps.json> --edge <edge script> --store <secrets store id> --app-id <id>
//     [--host-origin https://<host>] [--deny <zone>,...] [--old-seal <secret name>] [--upload-only]
// - `EDGE` is the service binding to the edge, the only way `graft-mcp` reaches the install.
// - `MCP_CLIENT_SECRET` and `MCP_SEAL_KEY` are Secrets Store bindings of the names `apps init` stored, with the
//   `secret_prefix` it recorded in apps.json; `--old-seal` binds `MCP_SEAL_KEY_OLD` during a seal-key rotation.
// - `MCP_CLIENT_ID` is `Graftyard MCP`'s client id for `--app-id`, the id `apps init --app-ids` registered it under;
//   `MCP_ORIGIN` is apps.json's
//   `mcp_origin`, else `https://mcp.<host>`; `HOST_ORIGIN` is `--host-origin`, else `https://<host>`; `DENY_ZONES` is the
//   host, the MCP origin's host, and `--deny`, so a CIMD URL may not name the install's own zones.
// - `VERSION` is the version metadata binding, the version report `graft-front` checks against `Pointers.mcp`.
// `--upload-only` uploads a version without deploying it (`wrangler versions upload`), for an `McpRoll`, which the ops
// path deploys by percentage. A product build (no test-faults: `graft-mcp` has none). Needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID; no
// secret value passes through this script.
//   node workers/mcp/deploy.mjs <name> --delete      wrangler delete
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const wrangler = (args, cwd) =>
  execFileSync("npx", ["--prefix", resolve(here, "../../tests/m0"), "wrangler", ...args], { cwd, stdio: "inherit" });
const args = process.argv.slice(2);
const flag = (f) => { const i = args.indexOf(f); return i >= 0 ? args[i + 1] : undefined; };
const [name, appsPath] = args;
if (!name) throw new Error("usage: deploy.mjs <name> <apps.json> --edge <script> --store <id> --app-id <id>");
if (appsPath === "--delete") {
  wrangler(["delete", "--name", name, "--force"], here);
  process.exit(0);
}
const [edge, store, appId] = [flag("--edge"), flag("--store"), flag("--app-id")];
if (!appsPath || !edge || !store || !/^\d+$/.test(appId ?? "")) {
  throw new Error("usage: deploy.mjs <name> <apps.json> --edge <script> --store <id> --app-id <id>");
}
const saved = JSON.parse(readFileSync(appsPath, "utf8"));
const prefix = saved.secret_prefix ?? "";
const origin = saved.mcp_origin ?? `https://mcp.${saved.host}`;
const hostOrigin = flag("--host-origin") ?? `https://${saved.host}`;
// `ClientId::for_app`: `Iv1.` and the App id as 16 hex digits.
const clientId = `Iv1.${BigInt(appId).toString(16).padStart(16, "0")}`;
const deny = [new URL(hostOrigin).host, new URL(origin).host, ...(flag("--deny")?.split(",") ?? [])].filter(Boolean);

const out = join(here, "build");
execFileSync("worker-build", ["--release", "--out-dir", out], { cwd: here, stdio: "inherit" });
const gen = join(here, ".gen");
mkdirSync(gen, { recursive: true });
const secret = (binding, n) => ({ binding, store_id: store, secret_name: n });
const cfg = {
  name,
  main: join(out, "index.js"),
  compatibility_date: "2026-10-01",
  // CIMD fetches go to clients' public URLs: route them as the Internet does. Without it a client_id on this
  // account's own workers.dev (the test installs' CIMD host) answers error 1042 (m3fa2, 2026-11-23), as M0-WP15's
  // graft-wp15-mcp set it.
  compatibility_flags: ["global_fetch_strictly_public"],
  workers_dev: true,
  preview_urls: false,
  services: [{ binding: "EDGE", service: edge }],
  version_metadata: { binding: "VERSION" },
  secrets_store_secrets: [
    secret("MCP_CLIENT_SECRET", `${prefix}MCP_CLIENT_SECRET_A`),
    secret("MCP_SEAL_KEY", `${prefix}MCP_SEAL_KEY_A`),
    ...(flag("--old-seal") ? [secret("MCP_SEAL_KEY_OLD", flag("--old-seal"))] : []),
  ],
  vars: { MCP_CLIENT_ID: clientId, MCP_ORIGIN: origin, HOST_ORIGIN: hostOrigin, DENY_ZONES: [...new Set(deny)].join(",") },
  observability: { enabled: true },
};
writeFileSync(join(gen, `${name}.jsonc`), JSON.stringify(cfg, null, 2));
wrangler([...(args.includes("--upload-only") ? ["versions", "upload"] : ["deploy"]), "-c", `${name}.jsonc`], gen);
