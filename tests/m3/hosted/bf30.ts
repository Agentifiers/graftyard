// BF-30 hosted on graft-m1 (`build/BUILD-PLAN.md`, M3-WP3): 20 transfers through `POST /repos/{o}/{r}/transfer`
// across enterprises, across placements (`Tri` [0,1,2] and `Na3` [0,3,1] home 3, sharing sites 0 and 1), and home
// only (`Tri` home 1), plus an org changing enterprise, which only `Audit` finishes. One transfer is sent with
// `x-graft-kill: after_reserved`, so the edge skips the in-band finish and `Audit`'s `Authz` cursor alone finishes it.
//
// During each transfer four routes are sampled back to back until it is done:
//   rOld, rNew: the repository chain's settings read through the old and the new owner's record, with the edge
//               isolate's cached configs dropped first (`/_bed/identity`, `cold`), as a new isolate would find it;
//   hId:        `GET /repositories/{id}/issues` over HTTPS (through `RepoOwner`);
//   hOld:       `GET /repos/{old}/{name}/issues` (the `Live` name, then its `Alias`).
// and `hNew`, `GET /repos/{new}/{name}/issues`, from its first 200 on (before `Live` it is 404 by design).
// After it is done: the chain's settings (what `deps` reads: `RepoFold::authz_owner` and `authz_enterprise`) name the
// new owner and its enterprise; a label write by the new owner's admin is accepted (201) and one by the old owner's
// admin is refused, both through `deps`.
//
// Secrets stay in memory; nothing is printed but ids, logins, statuses, and counts.
// m3-admin, the one destination every run shares, gets the repository as `<name>-<tag>` (`new_name`), so a run never
// meets an earlier run's repository there. `--only user` runs only the two transfers to it, from new repositories.
//   node tests/m3/hosted/bf30.ts --out ~/.graft-m3h --tag <a..z> --raw tests/m3/raw/finish/bf30 [--only user]

import { createHash, randomBytes } from "node:crypto";
import { appendFileSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: { out: { type: "string" }, tag: { type: "string" }, raw: { type: "string" }, only: { type: "string" } } });
if (!opt.out || !opt.tag || !opt.raw) throw new Error("usage: bf30.ts --out DIR --tag X --raw DIR");
mkdirSync(opt.raw, { recursive: true });
const ROOT = new URL("./", import.meta.url).pathname;
const TOOLS = process.env.GRAFT_TOOLS_EDGE ?? `https://graft-m1-wp7-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const EDGE = process.env.GRAFT_EDGE ?? `https://graft-m1-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const TOKEN = readFileSync(process.env.M0_TOKEN_FILE ?? `${ROOT}../../m0/.m0-token`, "utf8").trim();
const BED = readFileSync(`${process.env.HOME}/.graft-wp6/bed.token`, "utf8").trim();
const OP = { account: null, key: null };
const rec = (name: string, sites: number[], home: number) => ({ name, sites, home, generation: 1, moving_from: null, at: 5 });
const TRI = rec("Tri", [0, 1, 2], 0), NA3 = rec("Na3", [0, 3, 1], 3), TRI1 = rec("Tri", [0, 1, 2], 1);
const log = (m: string) => console.log(`${new Date().toISOString()} ${m}`);
const out = (file: string, v: unknown) => appendFileSync(`${opt.raw}/${file}`, JSON.stringify(v) + "\n");
const state = JSON.parse(readFileSync(`${opt.out}/m3.json`, "utf8"));
const ADMIN: number = state.admin;
const ADMIN_PAT = readFileSync(`${opt.out}/m3-tokens.txt`, "utf8").split("\n").find((l) => l.startsWith("admin "))!.slice(6);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

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
      await sleep(2000 * (i + 1));
    }
  }
}
const tools = (path: string, body: unknown) => post(TOOLS + path, body, { authorization: `Bearer ${TOKEN}` });
const bed = (body: unknown) => post(`${EDGE}/_bed/identity`, body, { "x-bed-token": BED });
function rid(): string {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = BigInt(Date.now()) & 0xffff_ffff_ffffn;
  for (const b of randomBytes(10)) v = (v << 8n) | BigInt(b);
  let s = "";
  for (let i = 25; i >= 0; i--) s += A[Number((v >> BigInt(5 * i)) & 31n)];
  return s;
}
const ownerChain = (chain: string) => chain.startsWith("authz:") || chain.startsWith("identity:");
async function commit(chain: string, cmd: unknown): Promise<any> {
  if (ownerChain(chain)) return (await bed({ chain, cmd })).outcome;
  await tools("/create", { chain });
  return (await tools("/commit", { chain, cmd, rid: rid() })).outcome;
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
const repoShard = (r: number) => nshard("graftyard/names/repo/", be64(r));

const base = 7_500_000 + (opt.tag.charCodeAt(0) - 97) * 1_000;
async function account(n: number, login: string, kind: string, record: unknown, enterprise: number | null = null): Promise<number> {
  const a = base + n;
  await must(`names:${accountShard(a)}`, { cred: OP, op: { PutAccount: { id: a, suspended: false, sessions_not_before: 0 } } });
  await must(`names:${loginShard(login)}`, { cred: OP, op: { ClaimLogin: { login, kind, account: a } } }, "Taken");
  await must(`names:${accountShard(a)}`, { cred: OP, op: { SetAccountLogin: { account: a, login } } });
  await must(`names:${accountShard(a)}`, { cred: OP, op: { InitPlacement: { owner: a, record } } }, "RecordExists");
  await must(`authz:${a}`, { cred: OP, op: { Open: { kind, enterprise } } }, "AlreadyOpened");
  if (kind === "User") await must(`identity:${a}`, { cred: OP, op: { Create: { login, kind, created: Date.now() } } }, "AlreadyCreated");
  return a;
}
// A fresh `ghp_` for `acct`, stored on its `Tokens` shard (tests/m3/hosted/newpat.mjs's shape), kept in memory.
async function pat(acct: number): Promise<string> {
  const A = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  let secret = "";
  while (secret.length < 30) for (const b of randomBytes(32)) if (b < 248 && secret.length < 30) secret += A[b % 62];
  let c = ~0;
  for (const ch of Buffer.from(secret)) { c ^= ch; for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1)); }
  let v = (~c) >>> 0;
  const tail = Array(6).fill("0");
  for (let i = 5; i >= 0; i--) { tail[i] = A[v % 62]; v = Math.floor(v / 62); }
  const h = createHash("sha256").update("graftyard/token/").update(secret).digest();
  await must(`tokens:${h.readUInt32BE(0) % 3072}`, { cred: OP, op: { Store: { hash: [...h], kind: "Pat", account: acct, scopes: 1, expires: null } } });
  return `ghp_${secret}${tail.join("")}`;
}
async function api(method: string, path: string, tok: string, body?: unknown, extra: Record<string, string> = {}) {
  const t0 = Date.now();
  try {
    const r = await fetch(`${EDGE}/api/v3${path}`, {
      method, headers: { authorization: `token ${tok}`, accept: "application/vnd.github+json", ...extra },
      body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(60_000), redirect: "manual",
    });
    const text = await r.text();
    let j: any = null;
    try { j = JSON.parse(text); } catch { /* not JSON */ }
    return { status: r.status, body: j, ms: Date.now() - t0 };
  } catch (e) {
    return { status: 0, body: String(e).slice(0, 120), ms: Date.now() - t0 };
  }
}
// One cold read of the repository chain's settings and config through `owner`'s record; `null` on any failure.
async function routeRead(repo: number, owner: number): Promise<{ owner: number; enterprise: number | null; voters: number[]; home: number } | null> {
  try {
    const r = await fetch(`${EDGE}/_bed/identity`, {
      method: "POST", headers: { "x-bed-token": BED }, signal: AbortSignal.timeout(30_000),
      body: JSON.stringify({ chain: `repo:${repo}`, read: { settings: true }, config: true, cold: true, record_of: owner }),
    });
    if (!r.ok) return null;
    const j = await r.json();
    if (!j.value) return null;
    const c = j.config?.config ?? {};
    const p = c.normal ?? c.joint?.new ?? c.degraded?.placement;
    const voters = [...new Set([...(c.joint?.old?.voters ?? []), ...(p?.voters ?? [])].map((v: any) => v.site))].sort();
    return { owner: j.value.owner, enterprise: j.value.enterprise, voters, home: p?.home };
  } catch {
    return null;
  }
}
async function readValue(chain: string, q: unknown): Promise<any> {
  if (ownerChain(chain)) return (await bed({ chain, read: q })).value;
  return (await tools("/read", { chain, q })).value;
}

// ---- the install's accounts ----
const tag = opt.tag;
const E1 = await account(1, `bf30${tag}-ent1`, "Enterprise", TRI);
const E2 = await account(2, `bf30${tag}-ent2`, "Enterprise", TRI);
type Org = { id: number; login: string; admin: number; pat: string; place: string; enterprise: number };
async function org(n: number, name: string, record: any, enterprise: number, place: string): Promise<Org> {
  const login = `bf30${tag}-${name}`;
  const id = await account(n, login, "Org", record, enterprise);
  const admin = await account(n + 100, `${login}-admin`, "User", TRI);
  await must(`authz:${id}`, { cred: OP, op: { SetMember: { user: admin, role: "Admin" } } });
  await must(`authz:${id}`, { cred: OP, op: { SetMember: { user: ADMIN, role: "Admin" } } });
  return { id, login, admin, pat: await pat(admin), place, enterprise };
}
const A = await org(11, "a", TRI, E1, "Tri"), B = await org(12, "b", TRI, E2, "Tri");
const C = await org(13, "c", NA3, E1, "Na3 home 3"), D = await org(14, "d", TRI1, E2, "Tri home 1");
const G = await org(15, "g", TRI, E1, "Tri");
const M3ADMIN: Org = { id: ADMIN, login: "m3-admin", admin: ADMIN, pat: ADMIN_PAT, place: "Tri", enterprise: 0 };
log(`accounts: E1 ${E1}, E2 ${E2}, a ${A.id}, b ${B.id}, c ${C.id}, d ${D.id}, g ${G.id}`);

async function createRepo(o: Org, name: string): Promise<number> {
  const r = await must(`authz:${o.id}`, { cred: OP, op: { CreateRepo: { name } } }, "Taken");
  const id: number = r?.ok?.out?.Repo?.repo ?? (await readValue(`authz:${o.id}`, { repo: name }))?.repo?.repo;
  await must(`names:${repoShard(id)}`, { cred: OP, op: { RepoOwner: { repo: id, owner: o.id } } });
  // The group is created on first use: a read through the edge.
  for (let i = 0; i < 5; i++) {
    const r = await api("GET", `/repositories/${id}/issues`, ADMIN_PAT);
    if (r.status === 200) break;
    log(`create ${o.login}/${name}: GET answered ${r.status}; retrying`);
    await sleep(3000);
  }
  return id;
}
const repos: Record<string, number> = {};
if (opt.only === "user") {
  for (const n of ["u1", "u2"]) repos[n] = await createRepo(D, n);
} else {
  for (let i = 1; i <= 9; i++) repos[`t${i}`] = await createRepo(A, `t${i}`);
  for (const n of ["g1", "g2"]) repos[n] = await createRepo(G, n);
}
log(`repos ${JSON.stringify(repos)}`);

// ---- one transfer ----
type Row = Record<string, unknown>;
let saved202 = false;
async function transfer(n: number, name: string, from: Org, to: Org, kind: string, kill = false): Promise<Row> {
  const repo = repos[name];
  const newName = to === M3ADMIN ? `${name}-${tag}` : name;
  const before = await routeRead(repo, from.id);
  const t0 = Date.now();
  const r = await api("POST", `/repos/${from.login}/${name}/transfer`, ADMIN_PAT, { new_owner: to.login, new_name: newName },
    kill ? { "x-graft-kill": "after_reserved" } : {});
  const row: Row = { n, name, newName, repo, kind, from: from.login, to: to.login, fromPlace: from.place, toPlace: to.place, kill, status: r.status, replyMs: r.ms, before };
  if (r.status !== 202) {
    row.error = r.body;
    log(`#${n} ${from.login}/${name} -> ${to.login}: ${r.status} ${JSON.stringify(r.body).slice(0, 200)}`);
    return row;
  }
  if (!saved202) {
    mkdirSync(`${opt.raw}/replies`, { recursive: true });
    writeFileSync(`${opt.raw}/replies/bf30-transfer.rest.json`, JSON.stringify({ method: "POST", path: "/repos/{owner}/{repo}/transfer", status: 202, body: r.body, location: null }, null, 1));
    saved202 = true;
  }
  // Sample every route back to back until the transfer is done, then once more.
  const s = { rOld: [0, 0], rNew: [0, 0], hId: [0, 0], hOld: [0, 0], hNew: [0, 0] } as Record<string, number[]>;
  const fails: unknown[] = [];
  const voterSets = new Set<string>();
  let done = false, doneMs = 0, newSeen = false;
  const note = (k: string, ok: boolean, detail?: unknown) => {
    s[k][ok ? 0 : 1]++;
    if (!ok && fails.length < 20) fails.push({ k, at: Date.now() - t0, detail });
  };
  const sampler = (async () => {
    let last = false;
    while (!last) {
      last = done;
      const [ro, rn, hi, ho, hn] = await Promise.all([
        routeRead(repo, from.id), routeRead(repo, to.id),
        api("GET", `/repositories/${repo}/issues`, ADMIN_PAT),
        api("GET", `/repos/${from.login}/${name}/issues`, ADMIN_PAT),
        api("GET", `/repos/${to.login}/${newName}/issues`, ADMIN_PAT),
      ]);
      note("rOld", ro !== null);
      note("rNew", rn !== null);
      for (const x of [ro, rn]) if (x) voterSets.add(`${x.voters.join(",")}h${x.home}`);
      note("hId", hi.status === 200, hi.status);
      note("hOld", ho.status === 200 || ho.status === 301, ho.status);
      if (hn.status === 200) newSeen = true;
      if (newSeen) note("hNew", hn.status === 200, hn.status);
    }
  })();
  // Done: `RepoOwner` names the new owner, the settings name it and its enterprise, the new name is `Live` and the old
  // one an `Alias`.
  const deadline = t0 + (kill ? 8 : 6) * 60_000;
  let last: Row = {};
  while (Date.now() < deadline) {
    await sleep(3000);
    try {
      const [ownerRow, settings, live, alias] = await Promise.all([
        readValue(`names:${repoShard(repo)}`, { repoOwner: repo }),
        routeRead(repo, to.id),
        readValue(`authz:${to.id}`, { repo: newName }),
        readValue(`authz:${from.id}`, { repo: name }),
      ]);
      const wantEnt = to.id === ADMIN ? null : to.enterprise;
      last = { repoOwner: ownerRow?.repoOwner, settings, live: live?.repo, alias: alias?.repo };
      if (ownerRow?.repoOwner === to.id && settings?.owner === to.id && settings?.enterprise === wantEnt
        && live?.repo?.repo === repo && live?.repo?.live === true && alias?.repo?.repo === repo && alias?.repo?.live === false) {
        done = true;
        doneMs = Date.now() - t0;
        break;
      }
    } catch (e) {
      last = { error: String(e).slice(0, 200) };
    }
  }
  done = true;
  await sampler;
  row.doneMs = doneMs || null;
  row.after = last;
  row.samples = s;
  row.fails = fails;
  row.configs = [...voterSets];
  // `deps` after `Settings`: the new owner's admin writes, the old owner's admin is refused.
  const labelBy = (o: Org, who: string) => api("POST", `/repos/${to.login}/${newName}/labels`, o.pat, { name: `bf30-${n}-${who}`, color: "ededed" });
  const [byNew, byOld] = [await labelBy(to, "new"), await labelBy(from, "old")];
  row.labelNew = byNew.status;
  row.labelOld = byOld.status;
  row.pass = doneMs > 0 && Object.values(s).every(([, bad]) => bad === 0) && s.hNew[0] > 0
    && byNew.status === 201 && [403, 404].includes(byOld.status);
  log(`#${n} ${kind} ${from.login}/${name} -> ${to.login}${kill ? " (killed after Reserved)" : ""}: ${row.pass ? "pass" : "FAIL"} done ${doneMs} ms, samples ${JSON.stringify(s)}, configs ${row.configs}, labels new ${byNew.status} old ${byOld.status}`);
  out("transfers.jsonl", row);
  return row;
}

const full: [string, Org, Org, string, boolean?][] = [
  ["t1", A, B, "enterprise"], ["t2", A, B, "enterprise"], ["t3", A, B, "enterprise"], ["t4", A, B, "enterprise"],
  ["t5", A, C, "placement"], ["t6", A, C, "placement"], ["t7", A, C, "placement"], ["t8", A, C, "placement"],
  ["t9", A, C, "placement", true],
  ["t5", C, B, "placement+enterprise"], ["t6", C, B, "placement+enterprise"], ["t7", C, B, "placement+enterprise"], ["t8", C, B, "placement+enterprise"],
  ["t1", B, D, "home"], ["t2", B, D, "home"], ["t3", B, D, "home"], ["t4", B, D, "home"],
  ["t1", D, M3ADMIN, "home, org to user"], ["t2", D, M3ADMIN, "home, org to user"],
  ["t9", C, A, "placement"],
];
const plan = opt.only === "user" ? [["u1", D, M3ADMIN, "home, org to user"], ["u2", D, M3ADMIN, "home, org to user"]] as typeof full : full;
const rows: Row[] = [];
for (const [i, [name, from, to, kind, kill]] of plan.entries()) rows.push(await transfer(i + 1, name, from, to, kind, kill ?? false));

// ---- the org's enterprise change: `Enterprise` on G's `Authz`, then `Audit` alone writes each repository's Settings
// (not with --only) ----
let ent: Record<string, unknown> = { pass: null };
if (opt.only === undefined) {
  const t0 = Date.now();
  await must(`authz:${G.id}`, { cred: OP, op: { SetEnterprise: { enterprise: E2 } } });
  let entDone = 0;
  const entSeen: unknown[] = [];
  while (Date.now() - t0 < 8 * 60_000) {
    await sleep(5000);
    const s = await Promise.all(["g1", "g2"].map((n) => routeRead(repos[n], G.id)));
    if (s.every((x) => x?.enterprise === E2 && x?.owner === G.id)) { entDone = Date.now() - t0; entSeen.push(s); break; }
  }
  const entLabels = await Promise.all(["g1", "g2"].map((n) => api("POST", `/repos/${G.login}/${n}/labels`, G.pat, { name: "bf30-ent", color: "ededed" })));
  ent = { org: G.login, from: E1, to: E2, doneMs: entDone || null, settings: entSeen, labels: entLabels.map((l) => l.status),
    pass: entDone > 0 && entLabels.every((l) => l.status === 201) };
  out("enterprise.jsonl", ent);
  log(`enterprise change ${G.login} E1 -> E2: ${ent.pass ? "pass" : "FAIL"} Settings on 2 of 2 after ${entDone} ms, labels ${ent.labels}`);
}

const pass = rows.filter((r) => r.pass).length;
const done = rows.filter((r) => r.doneMs).map((r) => r.doneMs as number).sort((a, b) => a - b);
const fails = rows.reduce((n, r) => n + Object.values((r.samples ?? {}) as Record<string, number[]>).reduce((m, [, b]) => m + b, 0), 0);
const total = rows.reduce((n, r) => n + Object.values((r.samples ?? {}) as Record<string, number[]>).reduce((m, [a, b]) => m + a + b, 0), 0);
const summary = { tag, transfers: rows.length, pass, routeSamples: total, routeFailures: fails,
  doneMedianMs: done[Math.floor(done.length / 2)] ?? null, doneMaxMs: done.at(-1) ?? null, enterprise: ent.pass };
out("summary.jsonl", summary);
log(`BF-30 ${JSON.stringify(summary)}`);
console.log("DONE");
