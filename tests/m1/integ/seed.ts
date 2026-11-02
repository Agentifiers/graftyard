// Seeds the real identity groups of the graft-m1-site install for the git edge's reruns (tests/m1/wp6 scripts):
// one owner account with a login, its `Authz` chain opened, its repositories created by `CreateRepo` with their
// `RepoOwner` rows, and four stored tokens: the owner (admin of its own account), a writer with a `Write` grant on
// every repository, a read-only user with a `Read` grant, and a writer whose token is then revoked. Every command
// goes through the M1-WP7 test edge (graft-m1-wp7-edge), which commits on any chain by the lookup rule, as the
// operator (no account), except `Open`, which only an operator submits anyway.
//
//   node tests/m1/integ/seed.ts --login wp6 --repos main,big,bf16 --out ~/.graft-integ
//
// Writes <out>/tokens.txt ("<token> <hash>" lines: owner, writer, read-only, revoked, as tests/m1/wp6/mktok.py does)
// and <out>/repos.json (name -> repository id). Idempotent: an existing login, record, repository, or token is kept.

import { createHash, randomBytes } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: {
  login: { type: "string", default: "wp6" }, repos: { type: "string", default: "main,big,bf16" },
  out: { type: "string" }, owner: { type: "string", default: "7000001" },
} });
if (!opt.out) throw new Error("usage: seed.ts --login L --repos a,b --out DIR [--owner ID]");
mkdirSync(opt.out, { recursive: true });
const ROOT = new URL("./", import.meta.url).pathname;
const EDGE = "https://graft-m1-wp7-edge.<workers-subdomain>.workers.dev";
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

async function commit(chain: string, cmd: unknown): Promise<any> {
  await edge("/create", { chain });
  const r = await edge("/commit", { chain, cmd, rid: rid() });
  if (!r.ok) throw new Error(`${chain}: ${JSON.stringify(r).slice(0, 400)}`);
  return r.outcome;
}

const be64 = (n: number) => { const b = Buffer.alloc(8); b.writeBigInt64BE(BigInt(n)); return b; };
const shard = (tag: string, body: Buffer) => createHash("sha256").update(tag).update(body).digest().readUInt32BE(0) % 1024;
const accountShard = (id: number) => shard("graftyard/names/account/", be64(id));
const loginShard = (login: string) => shard("graftyard/names/login/", Buffer.from(login.toLowerCase()));
const repoShard = (repo: number) => shard("graftyard/names/repo/", be64(repo));

/** A `ghp_` token, its stored hash, and its `Tokens` shard (`crates/identity`, `OpaqueToken`, `shard_of_hash`). */
function token(): { text: string; hash: number[]; shard: number } {
  const A = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  const secret = Array.from(randomBytes(30), (b) => A[b % 62]).join("");
  let crc = 0xffffffff;
  for (const c of Buffer.from(secret)) { crc ^= c; for (let k = 0; k < 8; k++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1)); }
  let v = (crc ^ 0xffffffff) >>> 0;
  const tail = Array(6).fill("0");
  for (let i = 5; i >= 0; i--) { tail[i] = A[v % 62]; v = Math.floor(v / 62); }
  const h = createHash("sha256").update("graftyard/token/").update(secret).digest();
  return { text: `ghp_${secret}${tail.join("")}`, hash: [...h], shard: h.readUInt32BE(0) % 3072 };
}

const owner = Number(opt.owner);
const users = { writer: owner + 1, reader: owner + 2, revoked: owner + 3 };
const ok = (o: any, ...allowed: string[]) => o?.ok || allowed.includes(o?.refused?.err?.split?.("(")[0] ?? o?.refused?.err);

// The owner and the three users: an account row each, and the owner's record and login.
for (const a of [owner, ...Object.values(users)]) {
  const o = await commit(`names:${accountShard(a)}`, { cred: OP, op: { PutAccount: { id: a, suspended: false, sessions_not_before: 0 } } });
  if (!ok(o)) throw new Error(`PutAccount ${a}: ${JSON.stringify(o)}`);
}
const placed = await commit(`names:${accountShard(owner)}`, { cred: OP, op: { InitPlacement: { owner, record: TRI } } });
if (!ok(placed, "RecordExists")) throw new Error(`InitPlacement: ${JSON.stringify(placed)}`);
const claimed = await commit(`names:${loginShard(opt.login!)}`, { cred: OP, op: { ClaimLogin: { login: opt.login, kind: "User", account: owner } } });
log(`login ${opt.login} on names:${loginShard(opt.login!)}: ${JSON.stringify(claimed)}`);
const authz = `authz:${owner}`;
const opened = await commit(authz, { cred: OP, op: { Open: { kind: "User", enterprise: null } } });
log(`open ${authz}: ${JSON.stringify(opened)}`);

const reposPath = `${opt.out}/repos.json`;
const repos: Record<string, number> = existsSync(reposPath) ? JSON.parse(readFileSync(reposPath, "utf8")) : {};
for (const name of opt.repos!.split(",")) {
  if (!repos[name]) {
    const o = await commit(authz, { cred: OP, op: { CreateRepo: { name } } });
    const id = o?.ok?.out?.Repo?.repo;
    if (!id) {
      const r = await edge("/read", { chain: authz, q: { repo: name } });
      repos[name] = r.value?.repo?.repo;
      if (!repos[name]) throw new Error(`CreateRepo ${name}: ${JSON.stringify(o)}`);
    } else repos[name] = id;
  }
  const id = repos[name];
  const ro = await commit(`names:${repoShard(id)}`, { cred: OP, op: { RepoOwner: { repo: id, owner } } });
  if (!ok(ro)) throw new Error(`RepoOwner ${id}: ${JSON.stringify(ro)}`);
  for (const [u, p] of [[users.writer, "Write"], [users.reader, "Read"], [users.revoked, "Write"]] as const) {
    const g = await commit(authz, { cred: OP, op: { SetGrant: { repo: id, user: u, permission: p } } });
    if (!ok(g)) throw new Error(`SetGrant ${id} ${u}: ${JSON.stringify(g)}`);
  }
  log(`repo ${opt.login}/${name}: ${id}`);
}
writeFileSync(reposPath, JSON.stringify(repos, null, 2));

const tokPath = `${opt.out}/tokens.txt`;
if (!existsSync(tokPath)) {
  const lines: string[] = [];
  for (const [who, acct] of [["owner", owner], ["writer", users.writer], ["reader", users.reader], ["revoked", users.revoked]] as const) {
    const t = token();
    const o = await commit(`tokens:${t.shard}`, { cred: OP, op: { Store: { hash: t.hash, kind: "Pat", account: acct, scopes: 1, expires: null } } });
    if (!ok(o)) throw new Error(`Store ${who}: ${JSON.stringify(o)}`);
    if (who === "revoked") {
      const r = await commit(`tokens:${t.shard}`, { cred: OP, op: { Revoke: { hash: t.hash } } });
      if (!ok(r)) throw new Error(`Revoke: ${JSON.stringify(r)}`);
    }
    lines.push(`${t.text} ${Buffer.from(t.hash).toString("hex")}`);
    log(`token ${who} (account ${acct}) on tokens:${t.shard}`);
  }
  writeFileSync(tokPath, lines.join("\n") + "\n", { mode: 0o600 });
}
log(`seeded: ${JSON.stringify(repos)}`);
