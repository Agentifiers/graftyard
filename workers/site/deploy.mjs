// Builds the site script once and deploys it as every site of an install, each binding every site's member and
// registry classes with `script_name` (design/DESIGN.md, Shape, "Members reach each other through Durable Object
// bindings"). A class must exist before another script binds it, so a site that is new to the install is first deployed
// binding only its own classes. Usage:
//   node workers/site/deploy.mjs <sites.json> [--genesis <genesis.json>] [--skip-build] [--test-faults] [--vars <vars.json>]
//                                [--image <digest-pinned image>] [--image-prev <image>] [--anchor <install.toml>] [--roll-shards <edge url>]
//                                [--ops <graft-ops script>] [--var K=V ...] [--only <site id>] [--down <site id>]
//                                [--audit-stream <Basin Pipelines stream id>] [--run-key <store id>/<secret name>]
//                                [--ssh <store id> --ssh-image <digest-pinned image> --edge <edge script>] [--ssh-off]
//                                [--export-bucket <bucket>]
// `--test-faults` builds with the `test-faults` feature, for test installs only; `--vars` adds variables to every site
// (the 24-hour run's GRAFTYARD_FAULTS and GRAFTYARD_REQUEST_RETENTION_S, and `GRAFTYARD_HOST`, the API host webhook
// payloads name). `--ops` binds the install's `graft-ops` as `OPS`, which `HookSequencer` unwraps hook secrets with.
// `--audit-stream <id>` binds that Basin Pipelines stream as `AUDIT_STREAM`, which the `Audit` consumer sends each
// scanned identity entry's rows to before it moves its cursor (workers/site/src/audit.rs).
// `--run-key <store id>/<secret name>` binds that Secrets Store secret as `RUN_SERVICE_KEY`, the run-service key the
// Actions leaders sign runner and job tokens with (decisions/DECISIONS.md, "M4-WP3: the run service's signing key").
// `--export-bucket <bucket>` turns the change feed on (design/DESIGN.md, Shape, "Lifeboat feed"): every site gets
// `GRAFTYARD_EXPORT=1`, so its leaders report commit indexes, and the install's first site binds the bucket as `EXPORT`
// and runs the rounds (workers/site/src/export_feed.rs). The bucket is created in that site's region if missing, with
// the 7-day lifecycle rule `expire7` on `export/`.
// `--var K=V` adds one more variable to every site deployed. `--only <id>` deploys that site alone (its phase 2
// config), leaving the others' versions and members untouched. `--down <id>` sets `GRAFTYARD_SITE_DOWN=1` on that
// site, which stops every class it hosts as if the site were lost (workers/site/src/lib.rs, `site_down`); the next
// deploy without it brings the site back.
// `--ssh <store id>` turns the SSH front on (design/DESIGN.md, Shape, "SSH front"): the script's entry becomes
// `entry.mjs`, which adds the JS classes `SshFront` and `SshOutbound` (`ssh.mjs`); `SshFront` (migration v6) gets the
// `sshfront` container from `--ssh-image` (or, without it, an image built from containers/ssh); the Secrets Store
// secrets `SSH_HOST_KEY_A` and `SSH_HOST_KEY_B` of that store are bound by the same names, which
// deploy/ssh-front/hostkeys.mjs writes; and `--edge <script>` binds the edge as `EDGE` with props `{"ssh": true}`, the
// only way a front's request reaches the edge as one. `--var SSH_HOST_KEY_ACTIVE=B` makes slot B the one key exchange
// serves (a rotation).
// A run without `--ssh` lists the account's Spectrum apps first and refuses while one has this install as its Worker
// origin, since the new versions would turn SSH off on that hostname; `--ssh-off` turns it off on purpose
// (`sshguard.mjs`).
// `--genesis` (test-faults installs only) names the system chains' records, which each site reads from
// GRAFTYARD_GENESIS in place of the `Release` view; a product install reads them from the genesis its `Release`
// group accepted, and needs `--anchor` (the trust anchor `graftyard init` wrote: GRAFTYARD_INSTALL and
// GRAFTYARD_CUSTOMER_KEY). Every site binds every site's bucket (`BUCKET_<n>`, created if missing) and hosts the
// git cache shard class `GitShard` on Cloudflare Containers, with `--image` (a digest-pinned image in the account's
// registry) or, without it, an image built from containers/git.
// Needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID. Every run creates a new version of each site script, which
// resets its members: outside a fresh install, that is a site day (AGENTS.md, invariants).

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { refuseSshOff } from "./sshguard.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const [sitesPath, ...flags] = process.argv.slice(2);
if (!sitesPath) throw new Error("usage: node workers/site/deploy.mjs <sites.json> [--genesis <genesis.json>] [--skip-build] [--test-faults] [--vars <vars.json>]");
const sites = JSON.parse(readFileSync(sitesPath, "utf8"));
// Without `--ssh`, refuse while a Spectrum app points at this install, unless `--ssh-off` (sshguard.mjs).
await refuseSshOff(flags, { siteScripts: sites.map((s) => s.script) });
const gi = flags.indexOf("--genesis");
const genesis = gi >= 0 ? JSON.parse(readFileSync(flags[gi + 1], "utf8")).placements : null;
const vi = flags.indexOf("--vars");
// Keys starting with `_` are comments.
const extraVars = Object.fromEntries([
  ...Object.entries(vi >= 0 ? JSON.parse(readFileSync(flags[vi + 1], "utf8")) : {}).filter(([k]) => !k.startsWith("_")),
  ...flags.flatMap((f, i) => (f === "--var" ? [flags[i + 1].split(/=(.*)/s).slice(0, 2)] : [])),
]);
const idFlag = (name) => (flags.indexOf(name) >= 0 ? Number(flags[flags.indexOf(name) + 1]) : null);
const only = idFlag("--only");
const down = idFlag("--down");
const ii = flags.indexOf("--image");
const image = ii >= 0 ? flags[ii + 1] : null;
// `--image-prev <digest-pinned image>`: the previous release's git image as `git_prev`, which a shard starts until its
// site's `ContainerDay` covers it (M5-WP4, `src/shard.rs`, `image`). Without it a shard asked for `git_prev` starts `git`.
const imagePrev = flags.indexOf("--image-prev") >= 0 ? flags[flags.indexOf("--image-prev") + 1] : null;
const si = flags.indexOf("--audit-stream");
const auditStream = si >= 0 ? flags[si + 1] : null;
const rki = flags.indexOf("--run-key");
const runKey = rki >= 0 ? flags[rki + 1].split("/") : null;
const flagValue = (name) => (flags.indexOf(name) >= 0 ? flags[flags.indexOf(name) + 1] : null);
const sshStore = flagValue("--ssh");
const sshImage = flagValue("--ssh-image");
const edgeScript = flagValue("--edge");
if (sshStore && !edgeScript) throw new Error("--ssh needs --edge <edge script>, which the SSH front's git requests go to");
const exportBucket = flagValue("--export-bucket");
const feedSite = Math.min(...sites.map((s) => s.id));
const oi = flags.indexOf("--ops");
const ops = oi >= 0 ? flags[oi + 1] : null;
const ai = flags.indexOf("--anchor");
const anchor = ai >= 0 ? Object.fromEntries(
  readFileSync(flags[ai + 1], "utf8").split("\n").map((l) => l.match(/^(install|customer_key)\s*=\s*"([^"]*)"/)).filter(Boolean).map((m) => [m[1], m[2]]),
) : null;
if (genesis && !flags.includes("--test-faults")) throw new Error("--genesis is a test-faults shortcut; a product install reads the Release view");
const COMPAT_DATE = "2026-10-01";
// The sweeper's Cron Trigger [guess]: deletes wait 24 h (ORPHAN_AFTER), so a quarter-hour cadence only bounds how late
// a missing `Committed` marker or third-bucket copy is repaired.
const SWEEP_CRON = "*/15 * * * *";
const gen = join(here, ".gen");
mkdirSync(gen, { recursive: true });

if (!flags.includes("--skip-build")) {
  const features = flags.includes("--test-faults") ? ["--features", "test-faults"] : [];
  execFileSync("worker-build", ["--release", ...features], { cwd: here, stdio: "inherit" });
}

const wrangler = (args) =>
  execFileSync("npx", ["--prefix", resolve(here, "../../tests/m0"), "wrangler", ...args], { cwd: gen, stdio: "inherit" });

// A site binds every site's `GitShard` class, its own first: Cloudflare attaches the container config to the first
// binding of the class, which must be the script's own. Another site's binding is how the archive alarm promotes on
// that site's shard while its own rehydrates (workers/site/src/archive.rs, `SiteHost::promote`).
// `HookSequencer` (v3), `SearchShard` and `Inbox` (v4), and `LogRelay` and `RunnerBroker` (v5) are new to scripts that
// exist, so phase 1 binds only a site's own; phase 2, once every site has the class, binds every site's.
const NEW_CLASSES = ["HOOKSEQ", "SEARCH", "INBOX", "LOGRELAY", "BROKER"];
function bindings(self, targets, phase1 = false) {
  const own = targets.filter((s) => s.id === self.id);
  const order = [...own, ...targets.filter((s) => s.id !== self.id)];
  return [["MEMBER", "Member"], ["REGISTRY", "Registry"], ["SHARD", "GitShard"], ["HOOKSEQ", "HookSequencer"],
    ["SEARCH", "SearchShard"], ["INBOX", "Inbox"], ["LOGRELAY", "LogRelay"], ["BROKER", "RunnerBroker"],
    // A site binds only its own SSH front: the edge picks the site, and the front never calls another's.
    ...(sshStore ? [["SSHFRONT", "SshFront"]] : [])].flatMap(([b, cls]) =>
    order.filter((s) => !(phase1 && NEW_CLASSES.includes(b) && s.id !== self.id) && !(b === "SSHFRONT" && s.id !== self.id)).map((s) => ({
      name: `${b}_${s.id}`,
      class_name: cls,
      ...(s.id === self.id ? {} : { script_name: s.script }),
    })),
  );
}

// `own`: the script exists, so it can bind itself; a site new to the install gets its `SITE` binding in phase 2.
function config(site, targets, own = true, phase1 = false) {
  return {
    name: site.script,
    // With SSH on, `entry.mjs` re-exports the Rust build and adds the SSH front's JS classes.
    main: sshStore ? "../entry.mjs" : "../build/index.js",
    compatibility_date: COMPAT_DATE,
    workers_dev: false,
    preview_urls: false,
    vars: {
      SITE_ID: String(site.id),
      GRAFTYARD_SITES: JSON.stringify(sites),
      ...(genesis ? { GRAFTYARD_GENESIS: JSON.stringify(genesis) } : {}),
      ...(anchor ? { GRAFTYARD_INSTALL: anchor.install, GRAFTYARD_CUSTOMER_KEY: anchor.customer_key } : {}),
      ...Object.fromEntries(Object.entries(extraVars).map(([k, v]) => [k, typeof v === "string" ? v : JSON.stringify(v)])),
      ...(site.id === down ? { GRAFTYARD_SITE_DOWN: "1" } : {}),
      ...(exportBucket ? { GRAFTYARD_EXPORT: "1" } : {}),
    },
    durable_objects: { bindings: bindings(site, targets, phase1) },
    // The version id each member's gate counters are kept under (M5-WP4, `src/gatecount.rs`).
    version_metadata: { binding: "VERSION" },
    // The script's own fetch handler, which runs each `PrSync` first attempt in a new invocation (prsync.rs, `in_band`),
    // and `--ops <name>`'s `graft-ops`, which unwraps hook secrets for `HookSequencer` (hookseq.rs).
    services: [
      ...(own ? [{ binding: "SITE", service: site.script }] : []),
      ...(ops ? [{ binding: "OPS", service: ops }] : []),
      ...(edgeScript && sshStore ? [{ binding: "EDGE", service: edgeScript, props: { ssh: true } }] : []),
    ],
    r2_buckets: sites.map((s) => ({ binding: `BUCKET_${s.id}`, bucket_name: s.bucket }))
      .concat(exportBucket && site.id === feedSite ? [{ binding: "EXPORT", bucket_name: exportBucket }] : []),
    ...((runKey || sshStore) ? { secrets_store_secrets: [
      ...(runKey ? [{ binding: "RUN_SERVICE_KEY", store_id: runKey[0], secret_name: runKey[1] }] : []),
      ...(sshStore ? ["SSH_HOST_KEY_A", "SSH_HOST_KEY_B"].map((n) => ({ binding: n, store_id: sshStore, secret_name: n })) : []),
    ] } : {}),
    ...(auditStream ? { pipelines: [{ binding: "AUDIT_STREAM", stream: auditStream }] } : {}),
    containers: [{
      class_name: "GitShard",
      scheduling_policy: "durable_object",
      images: {
        git: image ? { image } : { dockerfile: resolve(here, "../../containers/git/Dockerfile") },
        ...(imagePrev ? { git_prev: { image: imagePrev } } : {}),
      },
    },
    // The SSH front's one container per site, `graft-sshd` (containers/ssh), started by its `SshFront` object.
    ...(sshStore ? [{
      class_name: "SshFront",
      scheduling_policy: "durable_object",
      images: { ssh: sshImage ? { image: sshImage } : { dockerfile: resolve(here, "../../containers/ssh/Dockerfile") } },
    }] : [])],
    // One queue per site (design/DESIGN.md, Scale, fan-out): its repository leaders produce `PrSync` batches, and this
    // script consumes them (workers/site/src/prsync.rs). Batches of one message wait at most 0 s, so a head push's
    // batch is handled at once [guess]; Queues retries a failed message 10 times, 2 s apart.
    queues: {
      producers: [{ binding: "PRSYNC", queue: queueOf(site) }],
      consumers: [{ queue: queueOf(site), max_batch_size: 10, max_batch_timeout: 0, max_retries: 10, retry_delay: 2 }],
    },
    limits: { cpu_ms: 300000 },
    // The pack sweeper (workers/site/src/sweep.rs). Each site sweeps the networks it is home for.
    triggers: { crons: [SWEEP_CRON] },
    // v2 adds the git cache shard class to scripts that v1 created with members and registries only; v3 the owners'
    // webhook delivery stores (M4-WP1); v4 the search shards and the accounts' `Inbox` objects (M4-WP2); v5 the
    // Actions jobs' live-log relays and the runners' brokers (M4-WP3).
    migrations: [
      { tag: "v1", new_sqlite_classes: ["Member", "Registry"] },
      { tag: "v2", new_sqlite_classes: ["GitShard"] },
      { tag: "v3", new_sqlite_classes: ["HookSequencer"] },
      { tag: "v4", new_sqlite_classes: ["SearchShard", "Inbox"] },
      { tag: "v5", new_sqlite_classes: ["LogRelay", "RunnerBroker"] },
      // v6: the SSH front (M6-WP2), only on scripts that turn it on.
      ...(sshStore ? [{ tag: "v6", new_sqlite_classes: ["SshFront"] }] : []),
    ],
    observability: { enabled: true },
  };
}

// A Cloudflare API GET. The token reaches curl on stdin (`-H @-`), never argv, which `ps` shows.
const cfApi = (path) => execFileSync("curl", ["-sf", "-H", "@-",
  `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/${path}`],
  { input: `Authorization: Bearer ${process.env.CLOUDFLARE_API_TOKEN}\n` });

// Every site's bucket, in its site's location.
const buckets = JSON.parse(
  cfApi("r2/buckets?per_page=1000"),
).result.buckets.map((b) => b.name);
for (const s of sites.filter((s) => !buckets.includes(s.bucket))) {
  wrangler(["r2", "bucket", "create", s.bucket, "--location", s.region]);
}
if (exportBucket && !buckets.includes(exportBucket)) {
  wrangler(["r2", "bucket", "create", exportBucket, "--location", sites.find((s) => s.id === feedSite).region]);
  // Only the batches expire: `export-state` is rewritten only by a round that moved, so an install idle for 7 days
  // would otherwise lose its cursors.
  wrangler(["r2", "bucket", "lifecycle", "add", exportBucket, "expire7", "export/", "--expire-days", "7", "--force"]);
}

const listed = JSON.parse(
  cfApi("workers/scripts"),
).result.map((s) => s.id);

// Each site's queue, created if missing (`graft-m1-site-a` has `graft-m1-prsync-a`).
const queueOf = (s) => s.script.replace(/-site-/, "-prsync-");
const queues = JSON.parse(
  cfApi("queues?per_page=100"),
).result.map((q) => q.queue_name);
for (const s of sites.filter((s) => !queues.includes(queueOf(s)))) wrangler(["queues", "create", queueOf(s)]);

if (only !== null) {
  const site = sites.find((s) => s.id === only);
  if (!site || !listed.includes(site.script)) throw new Error(`--only ${only}: no deployed site with that id`);
  writeFileSync(join(gen, `${site.script}.jsonc`), JSON.stringify(config(site, sites), null, 2));
  wrangler(["deploy", "-c", `${site.script}.jsonc`]);
  process.exit(0);
}

// Phase 1: every site gets its own classes (a new site binds only itself; an existing one keeps its member and
// registry bindings to the sites that exist).
for (const site of sites) {
  const targets = listed.includes(site.script) ? sites.filter((s) => s === site || listed.includes(s.script)) : [site];
  writeFileSync(join(gen, `${site.script}.jsonc`), JSON.stringify(config(site, targets, listed.includes(site.script), true), null, 2));
  wrangler(["deploy", "-c", `${site.script}.jsonc`]);
}
for (const site of sites) {
  writeFileSync(join(gen, `${site.script}.jsonc`), JSON.stringify(config(site, sites), null, 2));
  wrangler(["deploy", "-c", `${site.script}.jsonc`]);
}

// `--roll-shards <edge url>` (test installs only): a new version restarts no running shard container, and
// `durable_object` containers take part in no Cloudflare rollout, so a shard moves to a new image only when its object
// restarts the container. In a product install that is the site's `ContainerDay` (design/DESIGN.md, "Git cache shards
// (derived)"), not built yet. Here every running instance whose image is not `--image` is torn down and caught up
// again through the test edge's `/_bed/kill` and `/_bed/catchup`, so it starts on the new image. Needs BED_TOKEN_FILE.
const ri = flags.indexOf("--roll-shards");
if (ri >= 0) {
  const edge = flags[ri + 1];
  if (!flags.includes("--test-faults") || !image) throw new Error("--roll-shards needs --test-faults and --image");
  const bed = readFileSync(process.env.BED_TOKEN_FILE, "utf8").trim();
  const api = (path) => JSON.parse(cfApi(`containers/${path}`)).result;
  const apps = api("applications");
  // KEEP_NETS (comma-separated network ids): a held network's shards keep running on their image, as
  // tests/fanout/teardown.py's KEEP_NETS leaves them (mt10's 1048603, held 24 h for a cost measurement).
  const keep = new Set((process.env.KEEP_NETS ?? "").split(",").filter(Boolean));
  for (const site of sites) {
    const app = apps.find((a) => a.name === `${site.script}-gitshard`);
    if (!app) continue;
    for (const i of api(`applications/${app.id}/instances?per_page=1000`).instances) {
      const m = i.name.match(/^git#(\d+)#net:(\d+)$/);
      if (!m || i.status.state !== "running" || i.image === image) continue;
      if (keep.has(m[2])) {
        console.log(`roll ${i.name}: kept (KEEP_NETS)`);
        continue;
      }
      const q = `repo=${m[2]}&shard=${m[1]}`;
      // A slow catchup (repo:65536's rehydrate took 140 s, and over 300 s while another ran) is reported and the roll
      // goes on, so one repository does not leave every later instance on the old image.
      const call = (route) => {
        try {
          return execFileSync("curl", ["-s", "-m", "600", "-X", "POST", "-H", "@-", "-o", "/dev/null", "-w", "%{http_code}",
            `${edge}/_bed/${route}?${q}`], { input: `x-bed-token: ${bed}\n` }).toString();
        } catch (e) {
          return `fail(${e.status})`;
        }
      };
      console.log(`roll ${i.name} from ${i.image.slice(-12)}: kill ${call("kill")}, catchup ${call("catchup")}`);
    }
  }
}
