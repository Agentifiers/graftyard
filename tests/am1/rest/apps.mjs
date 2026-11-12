// The install's own agent Apps (`design/AGENT-LAYER.md`, "First-party Apps"): `graftyard-workspace` and
// `graftyard-agent` (`AgentKind::GraftyardRun`) and `Graftyard MCP` (`AgentKind::Mcp`), each registered on the install
// account's `Authz(Genesis.account)` chain (`authz:1`) the way `tests/m3/hosted/triage.ts` registers an admin's App:
// the slug claim on `Names`, `PutApp` with a fresh RS256 key, `RegisterApp` and `RegisterClient`, then an
// installation on the org with its `RegisterInstallation`, then the `AgentApp` row (`PutAgentApp`) that L0-12, B0-36
// and B0-37 need for their Graftyard-run and MCP kinds (`tests/am1/results/am1-fix.md`, "Task 3").
//
// Stands in for `graftyard apps init` and `graftyard agents init` until AM2 builds them. Idempotent: a rerun finds
// each slug, record, route, installation and row and changes nothing. Each App's private key goes to
// <state>/agent-apps/<slug>.pem (0600), never to argv or a log; the state file gets the ids.
//
//   node tests/am1/rest/apps.mjs --plan     # print the commands for the current state; sends nothing
//   node tests/am1/rest/apps.mjs --emit ORG A B C  # the whole sequence as JSON, with App ids A, B, C (the slug
//                                                  # claims' answers); `crates/identity/tests/agent_apps.rs` commits it
//   node tests/am1/rest/apps.mjs            # register on the install GRAFT_EDGE / GRAFT_TOOLS_EDGE name
//
// Environment as `tests/am1/hosted/lib.mjs` (GRAFT_EDGE, GRAFT_TOOLS_EDGE, M3_STATE, BED_TOKEN_FILE).

import { createHash, generateKeyPairSync, createPrivateKey, createPublicKey } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";

const PLAN = process.argv.includes("--plan");
const EMIT = process.argv.indexOf("--emit");
const M3H = process.env.M3_STATE ?? `${process.env.HOME}/.graft-m3h`;
const statePath = `${M3H}/m3.json`;
const state = EMIT >= 0 ? {} : JSON.parse(readFileSync(statePath, "utf8"));

/** `install_account()`: `DbId::pack(0, 0, 1)`, account 1. */
export const INSTALL = 1;
/** Installation ids, beside setup.ts's 7190001 and triage.ts's 7190002. */
const INSTALLATIONS = { "graftyard-workspace": 7190011, "graftyard-agent": 7190012, "graftyard-mcp": 7190013 };

/** The table in AGENT-LAYER, "First-party Apps", plus the implied `metadata: read`. */
export const APPS = [
  { slug: "graftyard-workspace", name: "graftyard-workspace", kind: "graftyard_run", agentKind: "GraftyardRun",
    permissions: { contents: "Write", metadata: "Read" } },
  { slug: "graftyard-agent", name: "graftyard-agent", kind: "graftyard_run", agentKind: "GraftyardRun",
    permissions: { contents: "Read", issues: "Write", pull_requests: "Write", checks: "Read", statuses: "Read", actions: "Read", metadata: "Read" } },
  { slug: "graftyard-mcp", name: "Graftyard MCP", kind: "mcp", agentKind: "Mcp",
    permissions: { contents: "Write", issues: "Write", pull_requests: "Write", checks: "Read", statuses: "Read", actions: "Read", metadata: "Read" } },
];

const be64 = (n) => { const b = Buffer.alloc(8); b.writeBigInt64BE(BigInt(n)); return b; };
const nshard = (tag, body) => createHash("sha256").update(tag).update(body).digest().readUInt32BE(0) % 1024;
export const slugChain = (slug) => `names:${nshard("graftyard/names/app-slug/", Buffer.from(slug))}`;
export const appChain = (app) => `names:${nshard("graftyard/names/app/", be64(app))}`;
export const clientChain = (client) => `names:${nshard("graftyard/names/client/", Buffer.from(client))}`;
export const installationChain = (id) => `names:${nshard("graftyard/names/installation/", be64(id))}`;
export const clientOf = (app) => `Iv1.${app.toString(16).padStart(16, "0")}`;

/**
 * Every command after the slug claim, for one App whose id is `app` and whose public key is `der`, installed on
 * `org` as `inst`, in order: the record, the two `Names` routes, the installation's route and row, and the
 * `AgentApp` row. Pure, so `--plan` and the test below see what the hosted run sends.
 */
export function commands(a, app, der, org, inst, created) {
  const OP = { account: null, key: null };
  const client = clientOf(app);
  const record = {
    id: app, slug: a.slug, name: a.name, client, keys: [{ id: 1, public: [...der], created }], permissions: a.permissions,
    events: [], webhook: null, webhook_secret: null, client_secrets: [], callbacks: [], public: true,
  };
  return [
    { chain: `authz:${INSTALL}`, cmd: { cred: OP, op: { PutApp: { record } } } },
    { chain: appChain(app), cmd: { cred: OP, op: { RegisterApp: { app, owner: INSTALL } } } },
    { chain: clientChain(client), cmd: { cred: OP, op: { RegisterClient: { client, owner: INSTALL, app } } } },
    { chain: installationChain(inst), cmd: { cred: OP, op: { RegisterInstallation: { id: inst, app, owner: org } } } },
    { chain: `authz:${org}`, cmd: { cred: OP, op: { Install: { id: inst, row: { app, repos: "All", permissions: a.permissions }, app_owner: INSTALL } } } },
    { chain: `authz:${INSTALL}`, cmd: { cred: OP, op: { PutAgentApp: { app, kind: a.agentKind, confine: null } } } },
  ];
}

/** The App's key: the stored one, or a new 2048-bit RSA key written 0600 before anything is sent. */
function keyOf(slug) {
  const dir = `${M3H}/agent-apps`;
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const path = `${dir}/${slug}.pem`;
  if (!existsSync(path)) {
    const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
    writeFileSync(path, privateKey.export({ type: "pkcs1", format: "pem" }), { mode: 0o600, flag: "wx" });
  }
  return createPublicKey(createPrivateKey(readFileSync(path))).export({ type: "spki", format: "der" });
}

/**
 * An installation token for one of these Apps on the org (`POST /app/installations/{id}/access_tokens` with the
 * App's JWT, signed with its 0600 key); `extra` adds `repositories`, `permissions` or `graftyard_task`. For the
 * drivers: the token is registered with `secret` so `scan` refuses any log line that holds it.
 */
export async function agentToken(slug, extra = {}) {
  const { api, secret } = await import("../hosted/lib.mjs");
  const { createSign } = await import("node:crypto");
  const a = state.agentApps?.[slug];
  if (!a) throw new Error(`${slug} is not registered: run tests/am1/rest/apps.mjs first`);
  const pem = readFileSync(`${M3H}/agent-apps/${slug}.pem`, "utf8");
  const b64u = (b) => Buffer.from(b).toString("base64url");
  const now = Math.floor(Date.now() / 1000);
  const h = b64u(JSON.stringify({ alg: "RS256", typ: "JWT" }));
  const p = b64u(JSON.stringify({ iat: now - 30, exp: now + 540, iss: String(a.app) }));
  const jwt = secret(`${h}.${p}.${b64u(createSign("RSA-SHA256").update(`${h}.${p}`).sign(pem))}`);
  const r = await api("POST", `/api/v3/app/installations/${a.installation}/access_tokens`, null, extra, { authorization: `Bearer ${jwt}` });
  if (r.status === 201) secret(r.body.token);
  return r;
}

/** The install's chain opened as an org, then each App's slug claim and `commands`, as one JSON list. */
export function sequence(org, ids, der, created) {
  const OP = { account: null, key: null };
  const out = [{ chain: `authz:${INSTALL}`, cmd: { cred: OP, op: { Open: { kind: "Org", enterprise: null } } } }];
  APPS.forEach((a, i) => {
    out.push({ chain: slugChain(a.slug), cmd: { cred: OP, op: { ClaimSlug: { slug: a.slug, owner: INSTALL } } } });
    out.push(...commands(a, ids[i], der, org, INSTALLATIONS[a.slug], created));
  });
  return out;
}

async function main() {
  if (EMIT >= 0) {
    const [org, ...ids] = process.argv.slice(EMIT + 1, EMIT + 5).map(Number);
    const der = createPublicKey(generateKeyPairSync("rsa", { modulusLength: 2048 }).privateKey).export({ type: "spki", format: "der" });
    console.log(JSON.stringify(sequence(org, ids, der, 5)));
    return;
  }
  state.agentApps ??= {};
  const org = state.org;
  if (PLAN) {
    for (const a of APPS) {
      const app = state.agentApps[a.slug]?.app ?? 0;
      const cmds = commands(a, app, Buffer.from("spki"), org, INSTALLATIONS[a.slug], 0);
      console.log(`${a.slug}: ClaimSlug on ${slugChain(a.slug)}, then`);
      for (const c of cmds) console.log(`  ${c.chain} ${Object.keys(c.cmd.op)[0]}`);
    }
    return;
  }
  const { log, must, tools } = await import("../hosted/lib.mjs");
  // The install's chain is an org whose operator acts as its site admin; an install that already opened it says so.
  await must(`authz:${INSTALL}`, { cred: { account: null, key: null }, op: { Open: { kind: "Org", enterprise: null } } }, "AlreadyOpened");
  // The install's admin (m3-admin) is a site admin, an `Admin` of the install org, as tests/m3/hosted/rows.ts makes it:
  // `/api/graftyard/v1/agents/apps/{id}` needs one. `SetMember` again with the same role commits nothing new.
  await must(`authz:${INSTALL}`, { cred: { account: null, key: null }, op: { SetMember: { user: state.admin, role: "Admin" } } });
  for (const a of APPS) {
    const der = keyOf(a.slug);
    let app = state.agentApps[a.slug]?.app;
    if (!app) {
      const o = await must(slugChain(a.slug), { cred: { account: null, key: null }, op: { ClaimSlug: { slug: a.slug, owner: INSTALL } } }, "Taken");
      app = o?.ok?.out?.App ?? (await tools("/read", { chain: slugChain(a.slug), q: { slug: a.slug } })).value?.slug?.app;
      if (!app) throw new Error(`${a.slug}: the slug's App id is unknown`);
    }
    const inst = INSTALLATIONS[a.slug];
    // The key's time is kept, so a rerun sends the same record and `PutApp` answers Done with no event.
    const created = state.agentApps[a.slug]?.created ?? Date.now();
    state.agentApps[a.slug] = { app, installation: inst, client: clientOf(app), kind: a.kind, created };
    writeFileSync(statePath, JSON.stringify(state, null, 2));
    for (const c of commands(a, app, der, org, inst, created)) await must(c.chain, c.cmd);
    writeFileSync(statePath, JSON.stringify(state, null, 2));
    log(`${a.slug}: App ${app}, installation ${inst} on ${org}, AgentApp ${a.agentKind}`);
  }
}

if (import.meta.url === `file://${process.argv[1]}`) await main();
