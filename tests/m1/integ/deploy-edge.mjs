// Deploys the git edge (workers/edge) as graft-m1-edge against the site members of an install: one MEMBER_<n>,
// REGISTRY_<n>, and SHARD_<n> binding per site with script_name, and BUCKET_<n> per site. Usage:
//   node tests/m1/integ/deploy-edge.mjs <sites.json> <name> [--test-faults] [--genesis <genesis.json>]
//                                       [--anchor <install.toml>] [--genesis-signed <genesis.signed>] [--no-shard]
//                                       [--ops <graft-ops name>] [--var K=V ...] [--web <web/dist/client>]
//                                       [--upload-only] [--ssh <hostkeys public JSON>] [--ssh-off]
//                                       [--export-bucket <bucket>]
// `--export-bucket <bucket>` binds the install's export bucket as `EXPORT`, which `GET export/batches` lists
// (workers/edge/src/export.rs); the site deploy's flag of the same name creates it.
// `--genesis` (test-faults only) is the test install's stand-in for the Release view; `--anchor` sets the trust anchor
// a product edge accepts the Release group's genesis against. `--genesis-signed` (with `--anchor`) writes the committed
// signed genesis into GRAFTYARD_GENESIS_SIGNED, hex, after checking its SHA-256 is install.toml's `genesis` digest,
// so a cold isolate needs no Release read (design/DESIGN.md, "Install operations", Genesis, step 6). `--no-shard` leaves out SHARD_<n>, for site scripts
// with no `GitShard` class (git requests then fail; the release endpoint does not need a shard). `graftyard sites
// add` deploys the class, so an install it built needs no `--no-shard`. `--upload-only` uploads a version without
// deploying it (`wrangler versions upload`), so an `EdgeRoll` can deploy it beside N by percentage (M5-WP2).
// `--ssh <file>` turns native SSH on (M6-WP2): `SSH_OPTION=on`, one `SSHFRONT_<n>` binding per site to its `SshFront`
// class, which the edge's `connect()` handler opens, and `SSH_HOST_KEYS`, the public half of both host-key slots that
// deploy/ssh-front/hostkeys.mjs wrote, served at /api/graftyard/v1/meta/ssh. The sites must have been deployed with
// `--ssh` first, so the class exists.
// Without `--ssh`, the deploy lists the account's Spectrum apps and refuses while one has this edge (or a script
// binding this install's `SshFront`) as its Worker origin; `--ssh-off` turns SSH off on purpose
// (workers/site/sshguard.mjs).
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { refuseSshOff } from "../../../workers/site/sshguard.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../../..");
const [sitesPath, name, ...flags] = process.argv.slice(2);
if (!sitesPath || !name) throw new Error("usage: deploy-edge.mjs <sites.json> <name> [--test-faults] [--genesis g.json] [--anchor install.toml] [--genesis-signed g.signed] [--no-shard] [--var K=V]");
const sites = JSON.parse(readFileSync(sitesPath, "utf8"));
const flag = (f) => { const i = flags.indexOf(f); return i >= 0 ? flags[i + 1] : null; };
const tf = flags.includes("--test-faults");
const noShard = flags.includes("--no-shard");
const genesis = flag("--genesis") ? JSON.parse(readFileSync(flag("--genesis"), "utf8")).placements : null;
if (genesis && !tf) throw new Error("--genesis is a test-faults shortcut");
const anchorFile = flag("--anchor");
const anchor = anchorFile ? Object.fromEntries(readFileSync(anchorFile, "utf8").split("\n")
  .map((l) => l.match(/^(install|customer_key)\s*=\s*"([^"]*)"/)).filter(Boolean).map((m) => [m[1], m[2]])) : null;
// The committed genesis: install.toml's `genesis` is the SHA-256 of the signed bytes `graftyard install` sent and the
// Release group answered 201 for. Any other file is refused here, and the edge also accepts it only against the anchor.
const signedFile = flag("--genesis-signed");
let genesisSigned = null;
if (signedFile) {
  if (!anchorFile) throw new Error("--genesis-signed needs --anchor <install.toml>");
  const bytes = readFileSync(signedFile);
  const committed = (readFileSync(anchorFile, "utf8").match(/^genesis\s*=\s*"([0-9a-f]{64})"/m) || [])[1];
  const digest = createHash("sha256").update(bytes).digest("hex");
  if (!committed) throw new Error(`${anchorFile} names no committed genesis digest`);
  if (digest !== committed) throw new Error(`${signedFile} is not the committed genesis (${digest.slice(0, 8)}…, install.toml has ${committed.slice(0, 8)}…)`);
  genesisSigned = bytes.toString("hex");
}
// Without `--ssh`, refuse while a Spectrum app points at this edge, unless `--ssh-off` (workers/site/sshguard.mjs).
await refuseSshOff(flags, { edge: name, siteScripts: sites.map((s) => s.script) });
const extra = Object.fromEntries(flags.flatMap((f, i) => flags[i - 1] === "--var" ? [f.split(/=(.*)/s).slice(0, 2)] : []));

const edge = join(root, "workers/edge");
// A product build goes to its own directory, so it never replaces the test install's build under `build/`.
const out = tf ? join(edge, "build") : `/tmp/edge-prod-${name}`;
execFileSync("worker-build", ["--release", "--out-dir", out, ...(tf ? ["--features", "test-faults"] : [])], {
  cwd: edge, stdio: "inherit", env: { ...process.env, ...(tf ? {} : { CARGO_TARGET_DIR: "/tmp/edge-prod-target" }) },
});
const gen = join(here, ".gen");
mkdirSync(gen, { recursive: true });
const cfg = {
  name,
  main: join(out, "index.js"),
  compatibility_date: "2026-10-01",
  workers_dev: true,
  preview_urls: false,
  durable_objects: { bindings: sites.flatMap((s) => [["MEMBER", "Member"], ["REGISTRY", "Registry"], ["SHARD", "GitShard"], ["HOOKSEQ", "HookSequencer"], ["SEARCH", "SearchShard"], ["INBOX", "Inbox"], ["LOGRELAY", "LogRelay"], ["BROKER", "RunnerBroker"]]
    .concat(flag("--ssh") ? [["SSHFRONT", "SshFront"]] : [])
    .filter(([b]) => !(noShard && b === "SHARD")).map(([b, cls]) => ({ name: `${b}_${s.id}`, class_name: cls, script_name: s.script }))) },
  r2_buckets: sites.map((s) => ({ binding: `BUCKET_${s.id}`, bucket_name: s.bucket }))
    .concat(flag("--export-bucket") ? [{ binding: "EXPORT", bucket_name: flag("--export-bucket") }] : []),
  // `--web <dir>`: the web app's build (`npm run build` in web/, `VITE_MOCK_API=false`) as the `ASSETS` binding, with
  // the edge run first, so every path still reaches the edge (design/SHARED.md, "How the edge serves it").
  ...(flag("--web") ? { assets: { directory: resolve(flag("--web")), binding: "ASSETS", run_worker_first: true } } : {}),
  // `--ops <name>`: the `fetch()` binding to `graft-ops` that mints installation tokens (M3-WP4).
  ...(flag("--ops") ? { services: [{ binding: "OPS", service: flag("--ops") }] } : {}),
  // The version that ran, which the edge reports to `graft-front` (M5-WP2).
  version_metadata: { binding: "VERSION" },
  vars: {
    PLAN: "pro", SSH_OPTION: "off", EDGE_CAP_BYTES: "80000000", FORMATS_ENABLED: "32.1", PART_BYTES: "5242880",
    HOST: `${name}.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`,
    GRAFTYARD_SITES: JSON.stringify(sites),
    ...(genesis ? { GRAFTYARD_GENESIS: JSON.stringify(genesis) } : {}),
    ...(anchor ? { GRAFTYARD_INSTALL: anchor.install, GRAFTYARD_CUSTOMER_KEY: anchor.customer_key } : {}),
    ...(genesisSigned ? { GRAFTYARD_GENESIS_SIGNED: genesisSigned } : {}),
    ...(flag("--ops") ? { OPS_SCRIPT: flag("--ops") } : {}),
    ...(flag("--ssh") ? { SSH_OPTION: "on", SSH_HOST_KEYS: readFileSync(flag("--ssh"), "utf8").trim() } : {}),
    ...extra,
  },
  limits: { cpu_ms: 300000 },
  observability: { enabled: true },
};
writeFileSync(join(gen, `${name}.jsonc`), JSON.stringify(cfg, null, 2));
const cmd = flags.includes("--upload-only") ? ["versions", "upload"] : ["deploy"];
execFileSync("npx", ["--prefix", join(root, "tests/m0"), "wrangler", ...cmd, "-c", `${name}.jsonc`], { cwd: gen, stdio: "inherit" });
