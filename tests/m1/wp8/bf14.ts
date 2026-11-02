// BF-14 on the hosted test install: "Receipts that miss CALL_DEADLINE. Pass: zero commands forwarded to a second
// member." Each command's first `/commit` send carries a receipt fault (workers/site, `test-faults`): `delay` holds
// the member's reply 2.6 s, past the edge's COMMIT_RECEIPT of 2 s, and `drop` replaces it with a 503 once the command
// was admitted. The edge (graft-m1-edge, `graftyard_lookup::commit_hedged`) then hedges as it does in production.
//
//   node tests/m1/wp8/bf14.ts --run NAME [--push 600] [--identity 600] [--repos k0,...,k9] [--par 10]
//
// Two families: `Push` (`/_bed/push`, a single-ref push with no pack, under the writer's token) and identity
// (`/_bed/identity`: a `PutAccount` on a `Names` shard, committed by the operator through the same `commit_hedged`).
// Faults alternate `delay` and `drop`.
//
// The count, from two sources per command:
// - the edge's trace: every `/commit` send with its target and why the edge chose it: `lookup` (the first lookup's
//   answer, or a new lookup after `NotLeader` named a leader: `notLeader`, or after a lookup found none: `relookup`),
//   `same` (the previous target again), `suspectLed` (a voter that answered a `suspect` report as leader), or
//   `leaderless` (the next voter after `NotLeader` named none);
// - every member's `arrival` rows: each `/commit` that reached it, with its role on arrival and its answer.
// A command is forwarded if the edge sent it to a member other than its previous target for any reason but a
// redirect (below), or if a member got it more often than the edge sent it there (a member passing it on). Pass: 0.
// Rows go to tests/m1/wp8/raw/<run>.bf14.jsonl, so a run resumes.

import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { homedir } from "node:os";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: {
  run: { type: "string" }, push: { type: "string", default: "600" }, identity: { type: "string", default: "600" },
  repos: { type: "string", default: "k0,k1,k2,k3,k4,k5,k6,k7,k8,k9" }, par: { type: "string", default: "10" },
} });
if (!opt.run) throw new Error("usage: bf14.ts --run NAME [--push N] [--identity N]");
const ROOT = new URL("./", import.meta.url).pathname;
const RAW = `${ROOT}raw/`;
mkdirSync(RAW, { recursive: true });
const HOST = "https://graft-m1-edge.<workers-subdomain>.workers.dev";
const DIR = `${homedir()}/.graft-integ`;
const BED = readFileSync(`${homedir()}/.graft-wp6/bed.token`, "utf8").trim();
const WRITER = readFileSync(`${DIR}/tokens.txt`, "utf8").split("\n")[1].split(" ")[0];
const IDS: Record<string, number> = JSON.parse(readFileSync(`${DIR}/repos.json`, "utf8"));
const log = (m: string) => console.log(`${new Date().toISOString()} ${m}`);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const out = `${RAW}${opt.run}.bf14.jsonl`;
const rows = (): any[] => existsSync(out) ? readFileSync(out, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : [];

async function bed(path: string, q: string, body?: unknown, auth = false): Promise<any> {
  for (let i = 0; ; i++) {
    try {
      const headers: Record<string, string> = { "x-bed-token": BED };
      if (auth) headers.authorization = `token ${WRITER}`;
      const r = await fetch(`${HOST}/_bed/${path}?${q}`, {
        method: "POST", headers, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(180_000),
      });
      const t = await r.text();
      if (!r.ok) throw new Error(`${r.status} ${t.slice(0, 300)}`);
      return JSON.parse(t);
    } catch (e) {
      if (i >= 4) throw e;
      log(`${path} ${q}: ${String(e).slice(0, 200)}; retrying`);
      await sleep(3_000 * (i + 1));
    }
  }
}

const oid = (repo: number, i: number) => i < 0 ? null : createHash("sha1").update(`bf14/${opt.run}/${repo}/${i}`).digest("hex");
const ref = `refs/heads/bf14/${opt.run}`;
const mode = (i: number) => (i % 2 === 0 ? "delay" : "drop");
// `account_shard` (crates/identity): SHA-256 of the tag and the id's 8 bytes, mod 1024.
const accountShard = (id: number) => {
  const b = Buffer.alloc(8); b.writeBigInt64BE(BigInt(id));
  return createHash("sha256").update("graftyard/names/account/").update(b).digest().readUInt32BE(0) % 1024;
};
// Account ids for identity commands, one per command, far from the seeded ones, each on a `Names` shard that has a
// group: the shards of the seeded accounts (tests/m1/integ/seed.ts, owner 7000001 and its three users).
const shards = new Set([7000001, 7000002, 7000003, 7000004].map(accountShard));
const ACCOUNTS: number[] = [];
for (let id = 8_000_000 + (createHash("sha1").update(opt.run!).digest().readUInt16BE(0) << 12); ACCOUNTS.length < Number(opt.identity); id++) {
  if (shards.has(accountShard(id))) ACCOUNTS.push(id);
}
const account = (i: number) => ACCOUNTS[i];

const t0 = Date.now();
const started = rows().length ? rows()[0].at - 60_000 : t0 - 60_000;
const record = (r: any) => appendFileSync(out, JSON.stringify(r) + "\n");

async function onePush(name: string, i: number) {
  const repo = IDS[name];
  const r = await bed("push", `repo=${repo}`, { ref, expect: oid(repo, i - 1), new: oid(repo, i), receipt: mode(i) }, true)
    .catch((e) => ({ error: String(e) }));
  if (r.outcome && "Ok" in r.outcome) r.outcome = r.outcome.Ok;
  record({ fam: "push", name, repo, i, at: Date.now(), rid: r.rid, fault: mode(i), ms: r.ms, ok: r.outcome?.ok?.out?.refs?.[0] === "Ok",
    outcome: r.outcome ?? null, error: r.error ?? null, trace: r.trace ?? [] });
}

async function oneIdentity(i: number) {
  const id = account(i);
  const chain = `names:${accountShard(id)}`;
  const cmd = { cred: { account: null, key: null }, op: { PutAccount: { id, suspended: false, sessions_not_before: 0 } } };
  const r = await bed("identity", "", { chain, cmd, receipt: mode(i) }).catch((e) => ({ error: String(e) }));
  record({ fam: "identity", chain, i, at: Date.now(), rid: r.rid, fault: mode(i), ms: r.ms, ok: !!r.outcome?.ok,
    outcome: r.outcome ?? null, error: r.error ?? null, trace: r.trace ?? [] });
}

const done = rows();
const names = opt.repos!.split(",");
const nPush = Number(opt.push), nId = Number(opt.identity), par = Number(opt.par);
// Pushes: one lane per repository (each ref is a chain of expects); identity: `par` lanes.
const perRepo = Math.ceil(nPush / names.length);
const lanes: Promise<void>[] = names.map(async (name) => {
  let i = done.filter((r) => r.fam === "push" && r.name === name).length;
  while (i < perRepo) {
    await onePush(name, i);
    i++;
    if (i % 20 === 0) log(`${name}: ${i}/${perRepo}, ${Math.round((Date.now() - t0) / 1000)} s`);
  }
});
const idDone = new Set(done.filter((r) => r.fam === "identity").map((r) => r.i));
const idTodo = [...Array(nId).keys()].filter((i) => !idDone.has(i));
for (let l = 0; l < par; l++) {
  lanes.push((async () => {
    while (idTodo.length) {
      const i = idTodo.shift()!;
      await oneIdentity(i);
      if (i % 50 === 0) log(`identity: ${i}/${nId}, ${Math.round((Date.now() - t0) / 1000)} s`);
    }
  })());
}
await Promise.all(lanes);
log("commands done; reading every member's arrivals");

// Every member's arrivals since the run started: the repositories' members and the Names shards' members.
const all = rows();
const arrivals = new Map<string, any[]>();
const add = (site: number, name: string, list: any[]) => {
  for (const a of list ?? []) {
    const k = a.rid;
    if (!arrivals.has(k)) arrivals.set(k, []);
    arrivals.get(k)!.push({ site, name, ...a });
  }
};
const seen = new Set<string>();
for (const name of names) {
  for (const m of await bed("arrivals", `repo=${IDS[name]}&since=${started}`)) {
    if (seen.has(m.name)) continue;
    seen.add(m.name);
    add(m.site, m.name, m.rows);
  }
}
for (const chain of [...new Set(all.filter((r) => r.fam === "identity").map((r) => r.chain))]) {
  for (const m of await bed("identity_arrivals", `chain=${chain}&since=${started}`)) {
    if (seen.has(m.name)) continue;
    seen.add(m.name);
    add(m.site, m.name, m.rows);
  }
}

// Per command: the edge's sends, each with its reason, and the members it reached. A send to a member other than the
// first chosen is explained only by `notLeader` (a NotLeader redirect, through a new lookup that only a leader
// answers), `suspectLed` (a voter answered a suspect report as leader), `leaderless` (NotLeader naming no leader:
// the next voter, which refuses unless it leads), `relookup` (a lookup after one that found no leader, answered
// only by a leader), or `same` after one of those. An arrival with no send behind it, or more arrivals at a member
// than sends to it, is a command a member passed on. Either is forwarded.
const redirect = new Set(["notLeader", "suspectLed", "leaderless", "relookup"]);
let forwarded = 0, missesInTrace = 0, faultsLanded = 0, unexplainedSends = 0, extraArrivals = 0;
const reasons: Record<string, number> = {};
const perCmd: any[] = [];
for (const r of all) {
  const sends = (r.trace as any[]).filter((t) => "send" in t);
  for (const s of sends) reasons[s.why] = (reasons[s.why] ?? 0) + 1;
  missesInTrace += (r.trace as any[]).filter((t) => "missed" in t || "outcomeMissed" in t).length;
  const got = arrivals.get(r.rid) ?? [];
  faultsLanded += got.filter((a) => a.fault).length;
  const first = sends[0]?.name;
  let fwd = false;
  // The edge's own choices: a move to another member must follow a redirect.
  let prev = first;
  for (const s of sends.slice(1)) {
    if (s.name !== prev && !redirect.has(s.why)) { unexplainedSends++; fwd = true; }
    prev = s.name;
  }
  // The members' side: every arrival matches one of the edge's sends to that member.
  const sent = new Map<string, number>();
  for (const s of sends) sent.set(s.name, (sent.get(s.name) ?? 0) + 1);
  const arrived = new Map<string, number>();
  for (const a of got) arrived.set(a.name, (arrived.get(a.name) ?? 0) + 1);
  for (const [name, n] of arrived) {
    const k = n - (sent.get(name) ?? 0);
    if (k > 0) { extraArrivals += k; fwd = true; }
  }
  if (fwd) forwarded++;
  perCmd.push({ fam: r.fam, rid: r.rid, fault: r.fault, ok: r.ok, sends: sends.length, members: arrived.size,
    arrivals: got.length, faulted: got.filter((a) => a.fault).length, whys: sends.map((s) => s.why),
    roles: got.map((a) => `${a.site}:${a.role}:${a.answer}${a.fault ? ":" + a.fault : ""}`), forwarded: fwd });
}
const fam = (f: string) => {
  const xs = perCmd.filter((c) => c.fam === f);
  return { commands: xs.length, ok: xs.filter((c) => c.ok).length, faultLanded: xs.filter((c) => c.faulted > 0).length,
    secondMember: xs.filter((c) => c.members > 1).length, forwarded: xs.filter((c) => c.forwarded).length,
    byFault: Object.fromEntries(["delay", "drop"].map((m) => [m, xs.filter((c) => c.fault === m && c.faulted > 0).length])) };
};
const report = {
  run: opt.run, at: new Date().toISOString(), commands: all.length, ok: all.filter((r) => r.ok).length,
  missedReceiptsInTrace: missesInTrace, faultsLanded, sendsByReason: reasons, forwarded, unexplainedSends, extraArrivals,
  push: fam("push"), identity: fam("identity"),
  secondMemberExamples: perCmd.filter((c) => c.members > 1).slice(0, 10),
};
writeFileSync(`${RAW}${opt.run}.bf14-report.json`, JSON.stringify(report, null, 2));
log(JSON.stringify(report));
