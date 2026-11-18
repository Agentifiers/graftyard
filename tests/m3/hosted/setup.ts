// Seeds what journey 4 and BF-32 need on the graft-m1 install, through the M1-WP7 tools edge as the operator, as
// tests/m1/integ/seed.ts does:
// - an org `m3org` (account 7100001) with its record, login, `AccountLogin` and `Account` rows, an opened `Authz`
//   chain, and a private repository `j4` with its `RepoOwner` row;
// - users `m3-member` (7100002, an org member with the org's base `Write`) and `m3-admin` (7100003, the org's admin),
//   each with a login, an `AccountLogin` row, an `Account` row, and an `Identity` chain whose `Created` names it;
// - a GitHub App `m3-ci` owned by `m3-admin`, registered the M3-WP1 way (the slug claim, `PutApp`, the App and client
//   routes), with the RSA public key in <out>/app.pub.der and a client secret, and installed on `m3org` for
//   `contents: write` on every repository (`RegisterInstallation`, then `Install`);
// - one PAT each for `m3-member` and `m3-admin`, listed on their `Identity` chains.
// Writes <out>/m3.json (ids, the App id, the installation id, the client id) and <out>/m3-tokens.txt (mode 0600).
// The App's private key and client secret are made by `openssl` and `crypto` into <out>, mode 0600; no secret reaches
// argv or stdout. Idempotent: rows that exist are kept.
//
//   node tests/m3/hosted/setup.ts --out ~/.graft-m3h

import { createHash, generateKeyPairSync, randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: { out: { type: "string" } } });
if (!opt.out) throw new Error("usage: setup.ts --out DIR");
mkdirSync(opt.out, { recursive: true, mode: 0o700 });
const ROOT = new URL("./", import.meta.url).pathname;
const EDGE = process.env.GRAFT_TOOLS_EDGE ?? `https://graft-m1-wp7-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const TOKEN = readFileSync(`${ROOT}../../m0/.m0-token`, "utf8").trim();
const OP = { account: null, key: null };
const TRI = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 5 };
const log = (m: string) => console.log(`${new Date().toISOString()} ${m}`);

async function edge(path: string, body: unknown): Promise<any> {
  for (let i = 0; ; i++) {
    try {
      const r = await fetch(EDGE + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(300_000) });
      const t = await r.text();
      if (!r.ok) throw new Error(`${r.status} ${t.slice(0, 300)}`);
      return JSON.parse(t);
    } catch (e) {
      if (i >= 4) throw e;
      log(`${path}: ${String(e).slice(0, 200)}; retrying`);
      await new Promise((r) => setTimeout(r, 2000 * (i + 1)));
    }
  }
}

function rid(): string {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = BigInt(Date.now()) & 0xffff_ffff_ffffn;
  for (const b of randomBytes(10)) v = (v << 8n) | BigInt(b);
  let out = "";
  for (let i = 25; i >= 0; i--) out += A[Number((v >> BigInt(5 * i)) & 31n)];
  return out;
}

// `identity:` chains, which the tools edge has no record source for, go through the graft-m1 edge's bed-token
// `/_bed/identity` route, which finds an owner chain under its owner's `Placement` row.
const BED = process.env.GRAFT_EDGE ?? `https://graft-m1-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const BED_TOKEN = readFileSync(process.env.BED_TOKEN_FILE ?? `${process.env.HOME}/.graft-wp6/bed.token`, "utf8").trim();
async function bed(chain: string, cmd: unknown): Promise<any> {
  const r = await fetch(`${BED}/_bed/identity`, { method: "POST", body: JSON.stringify({ chain, cmd }), headers: { "x-bed-token": BED_TOKEN }, signal: AbortSignal.timeout(120_000) });
  const v = await r.json();
  if (!v.outcome) throw new Error(`${chain}: ${JSON.stringify(v).slice(0, 400)}`);
  return v.outcome;
}

async function commit(chain: string, cmd: unknown): Promise<any> {
  if (chain.startsWith("identity:")) return bed(chain, cmd);
  await edge("/create", { chain });
  const r = await edge("/commit", { chain, cmd, rid: rid() });
  if (!r.ok) throw new Error(`${chain}: ${JSON.stringify(r).slice(0, 400)}`);
  return r.outcome;
}
const refusal = (o: any): string | undefined => o?.refused?.err?.split?.(/[ ({]/)[0];
async function must(chain: string, cmd: unknown, ...allowed: string[]): Promise<any> {
  const o = await commit(chain, cmd);
  if (!o?.ok && !allowed.includes(refusal(o) ?? "")) throw new Error(`${chain} ${JSON.stringify(cmd).slice(0, 120)}: ${JSON.stringify(o).slice(0, 300)}`);
  return o;
}

const be64 = (n: number) => { const b = Buffer.alloc(8); b.writeBigInt64BE(BigInt(n)); return b; };
const shard = (tag: string, body: Buffer) => createHash("sha256").update(tag).update(body).digest().readUInt32BE(0) % 1024;
const accountShard = (id: number) => shard("graftyard/names/account/", be64(id));
const loginShard = (login: string) => shard("graftyard/names/login/", Buffer.from(login.toLowerCase()));
const repoShard = (repo: number) => shard("graftyard/names/repo/", be64(repo));
const keyShard = (tag: string, body: Buffer) => createHash("sha256").update(tag).update(body).digest().readUInt32BE(0) % 1024;

function token(): { text: string; hash: number[]; shard: number } {
  const A = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  let secret = "";
  while (secret.length < 30) for (const b of randomBytes(32)) if (b < 248 && secret.length < 30) secret += A[b % 62];
  const crc = (() => { let c = ~0; for (const ch of Buffer.from(secret)) { c ^= ch; for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1)); } return (~c) >>> 0; })();
  let v = crc;
  const tail = Array(6).fill("0");
  for (let i = 5; i >= 0; i--) { tail[i] = A[v % 62]; v = Math.floor(v / 62); }
  const h = createHash("sha256").update("graftyard/token/").update(secret).digest();
  return { text: `ghp_${secret}${tail.join("")}`, hash: [...h], shard: h.readUInt32BE(0) % 3072 };
}

const ORG = 7100001, MEMBER = 7100002, ADMIN = 7100003;
const accounts: [number, string, string][] = [[ORG, "m3org", "Org"], [MEMBER, "m3-member", "User"], [ADMIN, "m3-admin", "User"]];
for (const [a, login, kind] of accounts) {
  await must(`names:${accountShard(a)}`, { cred: OP, op: { PutAccount: { id: a, suspended: false, sessions_not_before: 0 } } });
  await must(`names:${loginShard(login)}`, { cred: OP, op: { ClaimLogin: { login, kind, account: a } } }, "Taken");
  await must(`names:${accountShard(a)}`, { cred: OP, op: { SetAccountLogin: { account: a, login } } });
  log(`${login} = ${a}`);
}
// Every owner of a chain needs a record: the org's `Authz`, and each user's `Identity`.
for (const [a] of accounts) await must(`names:${accountShard(a)}`, { cred: OP, op: { InitPlacement: { owner: a, record: TRI } } }, "RecordExists");
for (const [a, login, kind] of accounts) {
  await must(`authz:${a}`, { cred: OP, op: { Open: { kind, enterprise: null } } }, "AlreadyOpened");
  if (kind === "User") {
    await must(`identity:${a}`, { cred: OP, op: { Create: { login, kind, created: Date.now() } } }, "AlreadyCreated");
  }
}
const authz = `authz:${ORG}`;
await must(authz, { cred: OP, op: { SetMember: { user: ADMIN, role: "Admin" } } });
await must(authz, { cred: OP, op: { SetMember: { user: MEMBER, role: "Member" } } });
await must(authz, { cred: OP, op: { SetPolicy: { base_permission: "Write", sso_enforced: false } } });
const statePath = `${opt.out}/m3.json`;
const state: any = existsSync(statePath) ? JSON.parse(readFileSync(statePath, "utf8")) : {};
if (!state.repo) {
  const o = await commit(authz, { cred: OP, op: { CreateRepo: { name: "j4" } } });
  state.repo = o?.ok?.out?.Repo?.repo ?? (await edge("/read", { chain: authz, q: { repo: "j4" } })).value?.repo?.repo;
  if (!state.repo) throw new Error(`CreateRepo j4: ${JSON.stringify(o)}`);
}
await must(`names:${repoShard(state.repo)}`, { cred: OP, op: { RepoOwner: { repo: state.repo, owner: ORG } } });
log(`m3org/j4 = ${state.repo}`);

// The App, owned by m3-admin: RSA key and client secret made here, only the public half and the secret's hash sent.
const keyPath = `${opt.out}/app.pem`;
if (!existsSync(keyPath)) {
  const { privateKey } = generateKeyPairSync("rsa", { modulusLength: 2048 });
  writeFileSync(keyPath, privateKey.export({ type: "pkcs8", format: "pem" }), { mode: 0o600 });
  writeFileSync(`${opt.out}/app.secret`, randomBytes(20).toString("hex"), { mode: 0o600 });
}
const { createPrivateKey, createPublicKey } = await import("node:crypto");
const der = createPublicKey(createPrivateKey(readFileSync(keyPath))).export({ type: "spki", format: "der" });
const secretHash = [...createHash("sha256").update(readFileSync(`${opt.out}/app.secret`, "utf8")).digest()];
const slug = "m3-ci";
const slugChain = `names:${keyShard("graftyard/names/app-slug/", Buffer.from(slug))}`;
if (!state.app) {
  const o = await must(slugChain, { cred: OP, op: { ClaimSlug: { slug, owner: ADMIN } } }, "Taken");
  state.app = o?.ok?.out?.App ?? (await edge("/read", { chain: slugChain, q: { slug } })).value?.slug?.app;
  if (!state.app) throw new Error(`ClaimSlug: ${JSON.stringify(o)}`);
}
const app: number = state.app;
const client = `Iv1.${app.toString(16).padStart(16, "0")}`;
const perms = { contents: "Write", metadata: "Read" };
const record = {
  id: app, slug, name: "M3 CI", client, keys: [{ id: 1, public: [...der], created: Date.now() }], permissions: perms,
  events: [], webhook: null, webhook_secret: null, client_secrets: [secretHash], callbacks: [], public: true,
};
await must(`authz:${ADMIN}`, { cred: OP, op: { PutApp: { record } } });
await must(`names:${keyShard("graftyard/names/app/", be64(app))}`, { cred: OP, op: { RegisterApp: { app, owner: ADMIN } } });
await must(`names:${keyShard("graftyard/names/client/", Buffer.from(client))}`, { cred: OP, op: { RegisterClient: { client, owner: ADMIN, app } } });
state.installation ??= 7190001;
const inst = state.installation;
await must(`names:${keyShard("graftyard/names/installation/", be64(inst))}`, { cred: OP, op: { RegisterInstallation: { id: inst, app, owner: ORG } } });
await must(authz, { cred: OP, op: { Install: { id: inst, row: { app, repos: "All", permissions: perms }, app_owner: ADMIN } } });
log(`App ${slug} = ${app}, client ${client}, installed on m3org as ${inst}`);
Object.assign(state, { org: ORG, member: MEMBER, admin: ADMIN, client });
writeFileSync(statePath, JSON.stringify(state, null, 2));

const tokPath = `${opt.out}/m3-tokens.txt`;
if (!existsSync(tokPath)) {
  const lines: string[] = [];
  for (const [who, acct] of [["member", MEMBER], ["admin", ADMIN]] as const) {
    const t = token();
    await must(`tokens:${t.shard}`, { cred: OP, op: { Store: { hash: t.hash, kind: "Pat", account: acct, scopes: 1, expires: null } } });
    await must(`identity:${acct}`, { cred: OP, op: { AddCredential: { credential: { Token: { hash: t.hash, kind: "Pat" } }, title: `${who} PAT` } } });
    lines.push(`${who} ${t.text}`);
  }
  writeFileSync(tokPath, lines.join("\n") + "\n", { mode: 0o600 });
}
log("done");
