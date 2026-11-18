// M3-WP3's in-memory rows on the graft-m1 install, as far as its routes reach (`build/BUILD-PLAN.md`, M3-WP3):
// every command goes to the real hosted groups, through the M1-WP7 tools edge (system chains) and the graft-m1 edge's
// bed route (owner chains), and every result is read back from the groups. Fresh accounts in the 7300000 range,
// one set per run (`--tag`), so a rerun starts clean. Secrets stay in memory; nothing is printed but ids and counts.
//
//   BF-36: the install account's site admin (m3-admin, made an admin of the install org) sets `InstanceAuth` to
//          OIDC; m3-member and the operator are refused; the next sign-in by Password is `WrongMethod`, by OIDC and
//          by Link a session; then Builtin is restored.
//   BF-34 (org half): two invitations; one accepted (`Member` in the same entry), one declined (no member).
//   BF-30 (enterprise half): `SetEnterprise` from a user is refused; from the operator it commits and the fold names
//          the new enterprise.
//   BF-35: a new PAT and a new SSH key, each committed then listed by the edge's `after_credential` step, read from
//          the `Identity` chain at once, `n` times.
//   BF-3 (key half): one SSH key added for two accounts at once from two requests, `n` times: one owner each.
//
//   node tests/m3/hosted/rows.ts --out ~/.graft-m3h --tag <a..z> [--n 100]

import { execFileSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: { out: { type: "string" }, tag: { type: "string" }, n: { type: "string" } } });
if (!opt.out || !opt.tag) throw new Error("usage: rows.ts --out DIR --tag X [--n N]");
const N = Number(opt.n ?? 100);
const ROOT = new URL("./", import.meta.url).pathname;
const TOOLS = process.env.GRAFT_TOOLS_EDGE ?? `https://graft-m1-wp7-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const EDGE = process.env.GRAFT_EDGE ?? `https://graft-m1-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const TOKEN = readFileSync(`${ROOT}../../m0/.m0-token`, "utf8").trim();
const BED = readFileSync(process.env.BED_TOKEN_FILE ?? `${process.env.HOME}/.graft-wp6/bed.token`, "utf8").trim();
const OP = { account: null, key: null };
const TRI = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 5 };
const INSTALL = 1;
const log = (m: string) => console.log(`${new Date().toISOString()} ${m}`);
const state = JSON.parse(readFileSync(`${opt.out}/m3.json`, "utf8"));
const ADMIN: number = state.admin, MEMBER: number = state.member;
const as = (a: number) => ({ account: a, key: null });

async function post(url: string, body: unknown, headers: Record<string, string>): Promise<any> {
  for (let i = 0; ; i++) {
    try {
      const r = await fetch(url, { method: "POST", body: JSON.stringify(body), headers, signal: AbortSignal.timeout(300_000) });
      const t = await r.text();
      if (!r.ok) throw new Error(`${r.status} ${t.slice(0, 300)}`);
      return JSON.parse(t);
    } catch (e) {
      if (i >= 4) throw e;
      log(`${url.split("/").pop()}: ${String(e).slice(0, 200)}; retrying`);
      await new Promise((r) => setTimeout(r, 2000 * (i + 1)));
    }
  }
}
const tools = (path: string, body: unknown) => post(TOOLS + path, body, { authorization: `Bearer ${TOKEN}` });
const bed = (body: unknown) => post(`${EDGE}/_bed/identity`, body, { "x-bed-token": BED });
function rid(): string {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = BigInt(Date.now()) & 0xffff_ffff_ffffn;
  for (const b of randomBytes(10)) v = (v << 8n) | BigInt(b);
  let out = "";
  for (let i = 25; i >= 0; i--) out += A[Number((v >> BigInt(5 * i)) & 31n)];
  return out;
}
const owner = (chain: string) => chain.startsWith("authz:") || chain.startsWith("identity:");
async function commit(chain: string, cmd: unknown): Promise<any> {
  if (owner(chain)) return (await bed({ chain, cmd })).outcome;
  await tools("/create", { chain });
  return (await tools("/commit", { chain, cmd, rid: rid() })).outcome;
}
async function read(chain: string, q: unknown): Promise<any> {
  if (owner(chain)) return (await bed({ chain, read: q })).value;
  return (await tools("/read", { chain, q })).value;
}
const refusal = (o: any): string => o?.refused?.err?.split?.(/[ ({]/)[0] ?? (o?.ok ? "ok" : JSON.stringify(o).slice(0, 120));
async function must(chain: string, cmd: unknown, ...allowed: string[]): Promise<any> {
  const o = await commit(chain, cmd);
  if (!o?.ok && !allowed.includes(refusal(o))) throw new Error(`${chain} ${JSON.stringify(cmd).slice(0, 160)}: ${JSON.stringify(o).slice(0, 300)}`);
  return o;
}
const be64 = (n: number) => { const b = Buffer.alloc(8); b.writeBigInt64BE(BigInt(n)); return b; };
const nshard = (tag: string, body: Buffer) => createHash("sha256").update(tag).update(body).digest().readUInt32BE(0) % 1024;
const accountShard = (id: number) => nshard("graftyard/names/account/", be64(id));
const loginShard = (l: string) => nshard("graftyard/names/login/", Buffer.from(l.toLowerCase()));
const tshard = (h: Buffer) => h.readUInt32BE(0) % 3072;
const fpShard = (fp: Buffer) => tshard(createHash("sha256").update("graftyard/fingerprint/").update(fp).digest());

const base = 7_300_000 + (opt.tag.charCodeAt(0) - 97) * 1_000;
async function user(n: number, login: string, kind = "User"): Promise<number> {
  const a = base + n;
  await must(`names:${accountShard(a)}`, { cred: OP, op: { PutAccount: { id: a, suspended: false, sessions_not_before: 0 } } });
  await must(`names:${loginShard(login)}`, { cred: OP, op: { ClaimLogin: { login, kind, account: a } } }, "Taken");
  await must(`names:${accountShard(a)}`, { cred: OP, op: { SetAccountLogin: { account: a, login } } });
  await must(`names:${accountShard(a)}`, { cred: OP, op: { InitPlacement: { owner: a, record: TRI } } }, "RecordExists");
  await must(`authz:${a}`, { cred: OP, op: { Open: { kind, enterprise: null } } }, "AlreadyOpened");
  if (kind === "User") await must(`identity:${a}`, { cred: OP, op: { Create: { login, kind, created: Date.now() } } }, "AlreadyCreated");
  return a;
}
const result: Record<string, unknown> = { tag: opt.tag, n: N };
// An `Authz` fold's owner, enterprise, and roles, decoded from its `dep` read by `tools/src/bin/authzfold.rs`.
const AUTHZFOLD = `${process.env.CARGO_TARGET_DIR ?? `${ROOT}tools/target`}/debug/authzfold`;
async function authzFold(owner: number, ...users: number[]): Promise<any> {
  const hex = await read(`authz:${owner}`, { dep: true });
  return JSON.parse(execFileSync(AUTHZFOLD, users.map(String), { input: String(hex) }).toString());
}

// ---- BF-36 ----
{
  const install = `authz:${INSTALL}`;
  await must(install, { cred: OP, op: { SetMember: { user: ADMIN, role: "Admin" } } });
  await must(install, { cred: OP, op: { SetMember: { user: MEMBER, role: "Member" } } });
  const oidc = { Oidc: { issuer: "https://idp.graftyard.test/", client: "graftyard", secret: [1, 2, 3] } };
  const byMember = refusal(await commit(install, { cred: as(MEMBER), op: { SetInstanceAuth: oidc } }));
  const byOperator = refusal(await commit(install, { cred: OP, op: { SetInstanceAuth: oidc } }));
  const byAdmin = refusal(await commit(install, { cred: as(ADMIN), op: { SetInstanceAuth: oidc } }));
  const signIn = async (method: string) => refusal(await commit(`identity:${ADMIN}`, {
    cred: OP, op: { SignIn: { id: rid(), method, install: INSTALL } } }));
  const after = { Password: await signIn("Password"), Oidc: await signIn("Oidc"), Link: await signIn("Link") };
  const restore = refusal(await commit(install, { cred: as(ADMIN), op: { SetInstanceAuth: "Builtin" } }));
  const back = await signIn("Password");
  result.bf36 = { byMember, byOperator, byAdmin, after, restore, passwordAfterRestore: back };
  const pass = byMember === "Forbidden" && byOperator === "Forbidden" && byAdmin === "ok" && after.Password === "WrongMethod"
    && after.Oidc === "ok" && after.Link === "ok" && restore === "ok" && back === "ok";
  log(`BF-36 ${pass ? "pass" : "FAIL"} ${JSON.stringify(result.bf36)}`);
}

// ---- BF-34, the org half ----
{
  const org = await user(1, `m3r${opt.tag}-org`, "Org");
  const yes = await user(2, `m3r${opt.tag}-yes`), no = await user(3, `m3r${opt.tag}-no`);
  await must(`authz:${org}`, { cred: OP, op: { SetMember: { user: ADMIN, role: "Admin" } } });
  const invite = async (u: number) => (await must(`authz:${org}`, { cred: as(ADMIN), op: { Invite: { invitee: { User: u }, role: "Member", teams: [] } } })).ok.out.Invitation;
  const iy = await invite(yes), inn = await invite(no);
  const notYours = refusal(await commit(`authz:${org}`, { cred: as(no), op: { Accept: { id: iy } } }));
  const acc = await commit(`authz:${org}`, { cred: as(yes), op: { Accept: { id: iy } } });
  const dec = await commit(`authz:${org}`, { cred: as(no), op: { Decline: { id: inn } } });
  const again = refusal(await commit(`authz:${org}`, { cred: as(no), op: { Accept: { id: inn } } }));
  const f = await authzFold(org, yes, no);
  result.bf34 = { notYours, accepted: refusal(acc), declined: refusal(dec), acceptAfterDecline: again, roles: f.roles };
  result.bf34pass = notYours === "Forbidden" && refusal(acc) === "ok" && refusal(dec) === "ok" && again !== "ok"
    && f.roles[String(yes)] === "Member" && f.roles[String(no)] === null;
  log(`BF-34 org ${JSON.stringify(result.bf34)}`);

  // ---- BF-30, the enterprise half ----
  const ent = base + 9;
  const byUser = refusal(await commit(`authz:${org}`, { cred: as(ADMIN), op: { SetEnterprise: { enterprise: ent } } }));
  const byOp = refusal(await commit(`authz:${org}`, { cred: OP, op: { SetEnterprise: { enterprise: ent } } }));
  const same = await commit(`authz:${org}`, { cred: OP, op: { SetEnterprise: { enterprise: ent } } });
  const g = await authzFold(org);
  result.bf30 = { byUser, byOperator: byOp, resend: refusal(same), enterprise: g.enterprise, want: ent };
  result.bf30pass = byUser === "Forbidden" && byOp === "ok" && refusal(same) === "ok" && g.enterprise === ent;
  log(`BF-30 enterprise ${JSON.stringify(result.bf30)}`);
}

// ---- BF-35 ----
{
  const u = await user(4, `m3r${opt.tag}-keys`);
  let listed = 0, both = 0;
  const A = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  for (let i = 0; i < N; i++) {
    let secret = "";
    while (secret.length < 30) for (const b of randomBytes(32)) if (b < 248 && secret.length < 30) secret += A[b % 62];
    const h = createHash("sha256").update("graftyard/token/").update(secret).digest();
    await must(`tokens:${tshard(h)}`, { cred: OP, op: { Store: { hash: [...h], kind: "Pat", account: u, scopes: 1, expires: null } } });
    await must(`identity:${u}`, { cred: OP, op: { AddCredential: { credential: { Token: { hash: [...h], kind: "Pat" } }, title: `pat ${i}` } } });
    const fp = randomBytes(32);
    await must(`tokens:${fpShard(fp)}`, { cred: OP, op: { AddSshKey: { fingerprint: [...fp], account: u, repo: null } } });
    await must(`identity:${u}`, { cred: OP, op: { AddCredential: { credential: { SshKey: [...fp] }, title: `key ${i}` } } });
    const v = await read(`identity:${u}`, {});
    const creds: any[] = v?.credentials ?? [];
    const hasPat = creds.some((c) => c.token === h.toString("hex"));
    const hasKey = creds.some((c) => c.sshKey === fp.toString("hex"));
    if (hasPat) listed++;
    if (hasPat && hasKey) both++;
  }
  result.bf35 = { pairs: N, patListed: listed, bothListed: both };
  log(`BF-35 ${JSON.stringify(result.bf35)}`);
}

// ---- BF-3, the key half ----
{
  const u1 = await user(5, `m3r${opt.tag}-k1`), u2 = await user(6, `m3r${opt.tag}-k2`);
  let one = 0, taken = 0, bad: unknown[] = [];
  for (let i = 0; i < N; i++) {
    const fp = randomBytes(32);
    const chain = `tokens:${fpShard(fp)}`;
    await tools("/create", { chain });
    const add = (a: number) => tools("/commit", { chain, cmd: { cred: OP, op: { AddSshKey: { fingerprint: [...fp], account: a, repo: null } } }, rid: rid() });
    const [r1, r2] = await Promise.all([add(u1), add(u2)]);
    const oks = [r1.outcome, r2.outcome].filter((o) => o?.ok).length;
    const kt = [r1.outcome, r2.outcome].filter((o) => refusal(o) === "KeyTaken").length;
    const row = (await read(chain, { sshKey: fp.toString("hex") }))?.sshKey;
    if (oks === 1 && kt === 1 && row && [u1, u2].includes(row.account)) one++;
    else bad.push({ i, r: [refusal(r1.outcome), refusal(r2.outcome)], row });
    if (kt === 1) taken++;
  }
  result.bf3keys = { keys: N, oneOwner: one, keyTaken: taken, bad: bad.slice(0, 5) };
  log(`BF-3 keys ${JSON.stringify(result.bf3keys)}`);
}
console.log(JSON.stringify(result));
