// M1-WP7 gate runner on the deployed test install (graft-m1-site-a/b/c, edges graft-m1-wp7-edge and -b). Writes raw
// rows to tests/m1/wp7/raw/<run>.<row>.jsonl and resumes from them, so it can run as a supervised service.
//
//   node tests/m1/wp7/run.ts setup   --run NAME            # Names groups, owner records, the Ids group
//   node tests/m1/wp7/run.ts bf10    --run NAME [--chains 1000] [--conc 20]
//   node tests/m1/wp7/run.ts bf10mv  --run NAME [--chains 100] [--control 20]
//   node tests/m1/wp7/run.ts bf11    --run NAME [--groups 30] [--per 10]
//   node tests/m1/wp7/run.ts bf15    --run NAME [--queued 1000]
//   node tests/m1/wp7/run.ts report  --run NAME
//
// Owners are test accounts with ids from a test range, not assigned by `ClaimLogin`: each gets an operator
// `InitPlacement` (a copy of the install's Tri record) on `Names` shard `account_shard(owner)`, and its chain is
// `authz:<owner>`. The ids are picked so the owners fall on eight `Names` shards.

import { createHash, randomBytes } from "node:crypto";
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  run: { type: "string" }, chains: { type: "string" }, from: { type: "string" }, skip: { type: "string", default: "0" }, conc: { type: "string", default: "20" },
  control: { type: "string", default: "20" }, groups: { type: "string", default: "30" }, per: { type: "string", default: "10" },
  queued: { type: "string", default: "1000" },
} });
const row = positionals[0];
if (!opt.run || !row) throw new Error("usage: node tests/m1/wp7/run.ts setup|bf10|bf10mv|bf11|bf15|report --run NAME");
const ROOT = new URL("./", import.meta.url).pathname;
const RAW = `${ROOT}raw/`;
mkdirSync(RAW, { recursive: true });
const EDGES = ["https://graft-m1-wp7-edge.<workers-subdomain>.workers.dev", "https://graft-m1-wp7-edge-b.<workers-subdomain>.workers.dev"];
const TOKEN = readFileSync(`${ROOT}../../m0/.m0-token`, "utf8").trim();
const log = (m: string) => console.log(`${new Date().toISOString()} ${m}`);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const OP = { account: null, key: null };
const TRI = (gen: number, home = 0, at = 5) => ({ name: "Tri", sites: [0, 1, 2], home, generation: gen, moving_from: null, at });

async function edge<T = any>(which: 0 | 1, path: string, body: unknown, retry = true, ms = 600_000): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      const res = await fetch(EDGES[which] + path, {
        method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(ms),
      });
      const text = await res.text();
      if (!res.ok) throw new Error(`${res.status} ${text.slice(0, 300)}`);
      return JSON.parse(text) as T;
    } catch (e) {
      if (!retry || attempt >= 4) throw e;
      log(`${path} failed (${String(e).slice(0, 160)}); retrying`);
      await sleep(2_000 * (attempt + 1));
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

/** A commit through edge `which` under one request id; the edge re-sends it until it has an outcome, and a failed
 *  HTTP call is re-sent here under the same id. */
async function commit(which: 0 | 1, chain: string, cmd: unknown, extra: Record<string, unknown> = {}): Promise<any> {
  return edge(which, "/commit", { chain, cmd, rid: rid(), ...extra });
}

function accountShard(owner: number): number {
  const b = Buffer.alloc(8);
  b.writeBigInt64BE(BigInt(owner));
  return createHash("sha256").update("graftyard/names/account/").update(b).digest().readUInt32BE(0) % 1024;
}

/** Owners from a test range, on the first eight `Names` shards that range reaches. */
function owners(n: number, from: number): number[] {
  const shards: number[] = [];
  const out: number[] = [];
  for (let id = 5_000_000; out.length < from + n; id++) {
    const s = accountShard(id);
    if (shards.length < 8 && !shards.includes(s)) shards.push(s);
    if (shards.includes(s)) out.push(id);
  }
  return out.slice(from, from + n);
}

/** A token secret whose hash lands on `shard`, and its hash. */
function token(shard: number): { hash: number[]; shard: number } {
  const A = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  for (;;) {
    const secret = Array.from(randomBytes(30), (b) => A[b % 62]).join("");
    const h = createHash("sha256").update("graftyard/token/").update(secret).digest();
    if (h.readUInt32BE(0) % 3072 === shard) return { hash: [...h], shard };
  }
}

const file = (name: string) => `${RAW}${opt.run}.${name}.jsonl`;
const readRows = (name: string): any[] => existsSync(file(name))
  ? readFileSync(file(name), "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : [];
const put = (name: string, v: unknown) => appendFileSync(file(name), JSON.stringify(v) + "\n");

async function pool<T>(items: T[], conc: number, f: (x: T, i: number) => Promise<void>): Promise<void> {
  let next = 0;
  await Promise.all(Array.from({ length: conc }, async () => {
    while (next < items.length) { const i = next++; await f(items[i], i); }
  }));
}

// Owner pools, after [0, 3) (the first trial's): BF-10 part 1 and BF-11 [3, 1003), BF-10 during a move
// [1003, 1103), its control [1103, 1123), BF-15 [1123, 1125), trials [1125, 1150).
const POOL = { bf10: [3, 1000], mv: [1003, 100], control: [1103, 20], bf15: [1123, 2], trial: [1125, 25] } as const;

async function setup(): Promise<void> {
  const all = owners(1150, 0);
  const shards = [...new Set(all.map(accountShard))];
  log(`owners on names shards ${shards.join(",")}`);
  for (const s of [...shards.map((x) => `names:${x}`), "ids"]) {
    const r = await edge(0, "/create", { chain: s });
    log(`create ${s}: ${r.ok ? (r.existed ? "existed" : "created") : JSON.stringify(r).slice(0, 300)}`);
  }
  const done = new Set(readRows("setup").map((r) => r.owner));
  await pool(all.filter((o) => !done.has(o)), 16, async (o) => {
    const r = await commit(0, `names:${accountShard(o)}`, { cred: OP, op: { InitPlacement: { owner: o, record: TRI(1) } } });
    const ok = r.ok && (r.outcome?.ok || r.outcome?.refused?.err === "RecordExists");
    if (!ok) log(`InitPlacement ${o}: ${JSON.stringify(r).slice(0, 300)}`);
    else put("setup", { owner: o, shard: accountShard(o), outcome: r.outcome });
  });
  log(`setup: ${readRows("setup").length} owner records`);
}

/** Every member a chain's group may have, generations 1 to `gens`, and the chain's registry rows: one group means
 *  every member that holds a committed config holds the same bootstrap entry. */
async function census(chain: string, gens: number) {
  const c = await edge(0, "/census", { chain, gens });
  const configured = c.members.filter((m: any) => m.config !== null);
  const boots = [...new Set(configured.map((m: any) => m.boot1))];
  const gensBooted = [...new Set(configured.map((m: any) => m.gen))];
  return { groups: boots.length, configured: configured.length, joined: c.members.filter((m: any) => m.joined).length, gensBooted, members: c.members, rows: c.rows };
}

/** Both edges open the owner's `Authz` chain at once, each under its own request id: one group accepts exactly one. */
async function openRace(chain: string) {
  const [a, b] = await Promise.all([0, 1].map((w) => commit(w as 0 | 1, chain, { cred: OP, op: { Open: { kind: "User", enterprise: null } } }, { cold: true })));
  const outs = [a, b].map((r) => r.ok ? (r.outcome.ok ? "ok" : r.outcome.refused?.err) : `error: ${r.error}`);
  return { outs, accepted: outs.filter((x) => x === "ok").length, steps: [a.step, b.step] };
}

async function bf10(): Promise<void> {
  const n = Number(opt.chains ?? 1000);
  const list = owners(n, opt.run === "trial" ? POOL.trial[0] : POOL.bf10[0]);
  const done = new Set(readRows("bf10").map((r) => r.chain));
  await pool(list.filter((o) => !done.has(`authz:${o}`)), Number(opt.conc), async (o) => {
    const chain = `authz:${o}`;
    const t0 = Date.now();
    // Two edges create the chain at once: A campaigns on the record's home, B on site 2.
    const [a, b] = await Promise.all([
      edge(0, "/create", { chain, probe: true, cold: true }),
      edge(1, "/create", { chain, probe: true, cold: true, campaign_at: 2 }),
    ]);
    const open = await openRace(chain);
    const cen = await census(chain, 1);
    const probes = [...(a.probes ?? []), ...(b.probes ?? [])];
    const r = {
      chain, owner: o, ms: Date.now() - t0, created: [a.ok, b.ok], groups: cen.groups, configured: cen.configured, open,
      probes: probes.filter((p) => p.granted !== undefined).length,
      probeGranted: probes.filter((p) => p.granted === true).length,
      probeServed: probes.filter((p) => p.served === true).length,
      probeSkipped: probes.filter((p) => p.skipped !== undefined).length,
      denies: [...new Set(probes.map((p) => p.deny).filter(Boolean))],
      errors: [a, b].filter((x) => !x.ok).map((x) => JSON.stringify(x).slice(0, 400)),
    };
    put("bf10", r);
    if (r.groups !== 1 || open.accepted !== 1 || r.probeGranted || r.probeServed) log(`BF-10 ${chain}: ${JSON.stringify(r).slice(0, 600)}`);
  });
  summarize10("bf10");
}

/** One chain created by two edges while its owner's record changes from generation 1 (home 0) to 2 (home 1).
 *  Edge A holds the generation 1 record from its cache; edge B reads the record from `Names` after the move.
 *  - straddle: A's members join (and check their source) before the move commits; B creates after it; then A
 *    campaigns.
 *  - move-first: the move commits, then A and B create at once.
 *  - together: A creates while the move commits; B creates once it has.
 *  `no_claim` is the control: members check only that their record is the source's current generation. */
async function moveRound(o: number, mode: string, noClaim: boolean) {
  const chain = `authz:${o}`;
  const shard = `names:${accountShard(o)}`;
  const rec1 = TRI(1);
  const move = () => commit(0, shard, { cred: OP, op: { MovePlacement: { owner: o, name: "Tri", sites: [0, 1, 2], home: 1, at: 6 } } });
  const a = (extra: Record<string, unknown>) => edge(0, "/create", { chain, record: rec1, cold: true, lookup: false, no_claim: noClaim, ...extra });
  const b = async () => {
    const rec = (await edge(1, "/record", { chain, cold: true, fresh: true })).record;
    return { rec, r: await edge(1, "/create", { chain, record: rec, cold: true, no_claim: noClaim }) };
  };
  let mv: any, ra: any, rb: any;
  if (mode === "straddle") {
    ra = await a({ campaign: false });
    mv = await move();
    rb = await b();
    ra = { joins: ra.joins, campaign: await a({}) };
  } else if (mode === "move-first") {
    mv = await move();
    [ra, rb] = await Promise.all([a({}), b()]);
  } else {
    const pa = a({});
    mv = await move();
    rb = await b();
    ra = await pa;
  }
  await sleep(1_000);
  const open = await openRace(chain);
  const cen = await census(chain, 2);
  return {
    chain, owner: o, mode, noClaim, moved: mv.ok && !!mv.outcome?.ok, recordB: rb.rec?.generation ?? null,
    groups: cen.groups, gensBooted: cen.gensBooted, configured: cen.configured, open,
    refusedJoins: [ra.joins ?? ra.campaign?.joins ?? [], rb.r.joins ?? []].flat().filter((j: any) => j.status === 409).length,
  };
}

async function bf10mv(): Promise<void> {
  const modes = ["straddle", "move-first", "together"];
  const runs: [number, string, boolean][] = [
    ...owners(Number(opt.chains ?? 100), opt.run === "trial" ? POOL.trial[0] + 10 : POOL.mv[0]).map((o, i) => [o, modes[i % 3], false] as [number, string, boolean]),
    ...owners(Number(opt.control), opt.run === "trial" ? POOL.trial[0] + 20 : POOL.control[0]).map((o) => [o, "straddle", true] as [number, string, boolean]),
  ];
  const done = new Set(readRows("bf10mv").map((r) => r.chain));
  await pool(runs.filter(([o]) => !done.has(`authz:${o}`)), 10, async ([o, mode, noClaim]) => {
    const r = await moveRound(o, mode, noClaim);
    put("bf10mv", r);
    if ((r.groups !== 1 && !noClaim) || !r.moved) log(`BF-10 move ${r.chain}: ${JSON.stringify(r).slice(0, 600)}`);
  });
  summarize10("bf10mv");
}

function summarize10(name: string): void {
  const rows = readRows(name);
  const by = (f: (r: any) => boolean) => rows.filter(f).length;
  for (const control of [false, true]) {
    const rs = rows.filter((r) => !!r.noClaim === control);
    if (!rs.length) continue;
    log(`${name}${control ? " control (no claim)" : ""}: ${rs.length} chains, one group ${rs.filter((r) => r.groups === 1).length}, ` +
      `two groups ${rs.filter((r) => r.groups > 1).length}, none ${rs.filter((r) => r.groups === 0).length}, ` +
      `one Open accepted ${rs.filter((r) => r.open.accepted === 1).length}, two ${rs.filter((r) => r.open.accepted === 2).length}`);
  }
  if (name === "bf10") {
    log(`probes ${rows.reduce((s, r) => s + r.probes, 0)}, granted ${rows.reduce((s, r) => s + r.probeGranted, 0)}, ` +
      `served ${rows.reduce((s, r) => s + r.probeServed, 0)}, skipped (had a config) ${rows.reduce((s, r) => s + r.probeSkipped, 0)}; ` +
      `creates failed ${by((r) => r.errors.length > 0)}`);
  }
}

/** Cold-cache requests on one chain, alternating edges and reads with operator `SetGrant` commits. Every third
 *  request also skips the bootstrap names of the first record, so it must find the group through the registry or
 *  `moving_from` (`skip` adds steps to skip for every request). */
async function coldRequests(chain: string, owner: number, phase: string, per: number, base: string[] = []) {
  const out: any[] = [];
  for (let i = 0; i < per; i++) {
    const which = (i % 2) as 0 | 1;
    const skip = i % 3 === 2 ? [...base, "bootstrap0"] : base;
    const t0 = Date.now();
    const r = i % 4 < 2
      ? await edge(which, "/read", { chain, q: {}, cold: true, skip })
      : await commit(which, chain, { cred: OP, op: { SetGrant: { repo: 1, user: owner, permission: "Read" } } }, { cold: true, skip });
    out.push({ chain, phase, i, edge: which, skip, kind: i % 4 < 2 ? "read" : "commit", found: !!r.ok, step: r.step ?? null, ms: Date.now() - t0, error: r.ok ? undefined : r.error });
  }
  return out;
}

async function bf11(): Promise<void> {
  const ng = Number(opt.groups), per = Number(opt.per);
  // BF-11 runs on chains BF-10 created (`--from` names that run), skipping the first `--skip`.
  const bf10rows = existsSync(`${RAW}${opt.from ?? opt.run}.bf10.jsonl`)
    ? readFileSync(`${RAW}${opt.from ?? opt.run}.bf10.jsonl`, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : [];
  const done = bf10rows.filter((r) => r.groups === 1 && r.open.accepted === 1).map((r) => r.owner).slice(Number(opt.skip), Number(opt.skip) + ng);
  if (done.length < ng) throw new Error(`BF-11 needs ${ng} BF-10 chains; ${done.length} done`);
  const have = new Set(readRows("bf11ops").map((r) => r.chain));
  await pool(done.filter((o) => !have.has(`authz:${o}`)), 6, async (o) => {
    const chain = `authz:${o}`;
    const ops: any = { chain, owner: o };
    const reqs: any[] = [];
    const leaderCall = async (path: string, body: unknown) => {
      const l = await edge(0, "/leader", { chain, cold: true });
      return edge(0, "/member", { chain, site: l.leader.site, do_name: l.leader.do_name, path, ...(body === undefined ? {} : { body }), deadline: 150_000 });
    };
    // A member is replaced only off the leader's site: transfer away first, as the design's rule says.
    const offLeader = async (site: number) => {
      const l = await edge(0, "/leader", { chain, cold: true });
      if (l.leader.site !== site) return null;
      const t = await leaderCall("/transfer", { to: site === 0 ? 1 : 0 });
      await sleep(3_000);
      return t.v;
    };
    reqs.push(...await coldRequests(chain, o, "before", per));
    // 1. Member retry: site 2's member is replaced, and its first replacement fails its latency check (injected),
    //    so the second is the next generation again.
    ops.moveOff2 = await offLeader(2);
    ops.retry = (await leaderCall("/replace", { site: 2, fail_first: true })).v;
    reqs.push(...await coldRequests(chain, o, "retry", per));
    // 2. ReplaceMember (stub): site 1's member is lost (its storage deleted), then replaced on the same site.
    ops.lost = (await edge(0, "/member", { chain, site: 1, gen: 1, path: "/retire", body: {} })).status;
    ops.moveOff1 = await offLeader(1);
    ops.replace = (await leaderCall("/replace", { site: 1 })).v;
    reqs.push(...await coldRequests(chain, o, "replace", per));
    // 3. Owner move: the record goes to generation 2 with home 1; the group's leadership moves to the new home;
    //    requests first find it through `moving_from` (the registry step skipped), then `FinishMove` clears it.
    const shard = `names:${accountShard(o)}`;
    ops.move = (await commit(0, shard, { cred: OP, op: { MovePlacement: { owner: o, name: "Tri", sites: [0, 1, 2], home: 1, at: 7 } } })).outcome;
    if (!ops.move?.ok) throw new Error(`BF-11 ${chain}: move refused ${JSON.stringify(ops.move)}`);
    // The group's config takes the new record's placement: the same members (the move keeps the sites), home 1.
    const st = (await leaderCall("/status", undefined)).v;
    const placement = st.config.config.normal;
    ops.rehome = (await leaderCall("/reconfigure", { config: { normal: { ...placement, home: 1 } } })).v;
    ops.transfer = (await leaderCall("/transfer", { to: 1 })).v;
    await sleep(2_000);
    reqs.push(...await coldRequests(chain, o, "moving", per, ["registry@0"]));
    ops.finish = (await commit(0, shard, { cred: OP, op: { FinishMove: { owner: o, generation: 2 } } })).outcome;
    reqs.push(...await coldRequests(chain, o, "moved", per));
    await sleep(20_000);
    ops.leaderAfter = (await edge(0, "/leader", { chain, cold: true })).leader;
    ops.census = await census(chain, 5);
    for (const r of reqs) put("bf11", r);
    put("bf11ops", { ...ops, census: { groups: ops.census.groups, members: ops.census.members.filter((m: any) => m.joined) } });
    const miss = reqs.filter((r) => !r.found);
    log(`BF-11 ${chain}: ${reqs.length - miss.length}/${reqs.length} found${miss.length ? ` ${JSON.stringify(miss[0]).slice(0, 300)}` : ""}`);
  });
  const rows = readRows("bf11");
  const phases = [...new Set(rows.map((r) => r.phase))];
  for (const p of phases) {
    const rs = rows.filter((r) => r.phase === p);
    const steps: Record<string, number> = {};
    for (const r of rs) steps[r.step ?? "none"] = (steps[r.step ?? "none"] ?? 0) + 1;
    log(`BF-11 ${p}: ${rs.filter((r) => r.found).length}/${rs.length} found; steps ${JSON.stringify(steps)}`);
  }
}

async function bf15(): Promise<void> {
  const [ownR, ownS] = owners(2, opt.run === "trial" ? POOL.trial[0] + 23 : POOL.bf15[0]);
  const queued = Number(opt.queued);
  const TSHARD = 2279;
  for (const [o, how] of [[ownR, "revoke"], [ownS, "suspend"]] as const) {
    if (readRows("bf15").some((r) => r.how === how)) continue;
    const chain = `authz:${o}`, names = `names:${accountShard(o)}`, tokens = `tokens:${TSHARD}`;
    await edge(0, "/create", { chain: tokens });
    const put1 = await commit(0, names, { cred: OP, op: { PutAccount: { id: o, suspended: false, sessions_not_before: 0 } } });
    const created = await edge(0, "/create", { chain });
    const opened = await commit(0, chain, { cred: OP, op: { Open: { kind: "User", enterprise: null } } });
    const t = token(TSHARD);
    const stored = await commit(0, tokens, { cred: OP, op: { Store: { hash: t.hash, kind: "Pat", account: o, scopes: 0, expires: null } } });
    const cred = { account: o, key: { Token: { shard: t.shard, hash: t.hash } } };
    const cmd = (i: number) => ({ cred, op: { SetGrant: { repo: 1 + i, user: o, permission: "Write" } } });
    // The credential works before the change.
    const before = await commit(0, chain, cmd(100_000));
    log(`BF-15 ${how}: account ${put1.outcome?.ok ? "ok" : JSON.stringify(put1)}, created ${created.ok}, opened ${JSON.stringify(opened.outcome)}, stored ${JSON.stringify(stored.outcome)}, before ${JSON.stringify(before.outcome)}`);
    if (!put1.outcome?.ok || !created.ok || !stored.outcome?.ok || !before.outcome?.ok) throw new Error(`BF-15 ${how}: setup failed`);
    // `queued` commands reach the leader in one call; it holds its first dependency read for 8 s, so they queue
    // behind the first entry, and the change commits 2 s in.
    const t0 = Date.now();
    const batch = edge(0, "/commit_many", { chain, cmds: Array.from({ length: queued }, (_, i) => cmd(i)), hold_deps_ms: 8_000, hold_ms: 120_000 }, false);
    await sleep(2_000);
    const change = how === "revoke"
      ? await commit(1, tokens, { cred: OP, op: { Revoke: { hash: t.hash } } })
      : await commit(1, names, { cred: OP, op: { PutAccount: { id: o, suspended: true, sessions_not_before: 0 } } });
    const changedAt = Date.now() - t0;
    const r = await batch;
    const results: any[] = r.results ?? [];
    const outs = results.map((x) => x.done?.ok ? "accepted" : x.done?.refused ? `${x.done.refused.status} ${x.done.refused.err}` : x.admitted ? "undecided" : JSON.stringify(x).slice(0, 100));
    const counts: Record<string, number> = {};
    for (const x of outs) counts[x] = (counts[x] ?? 0) + 1;
    const indexes = [...new Set(results.map((x) => x.done?.refused?.index ?? x.done?.ok?.index).filter((x) => x !== undefined))];
    const deps = await edge(0, "/member", { chain, site: r.leader.site, do_name: r.leader.do_name, path: "/deps_log" });
    const after = (deps.v as any[]).filter((d) => d.at >= t0);
    const row = {
      how, owner: o, queued, changeIndex: change.outcome?.ok?.index ?? null, changedAtMs: changedAt, counts,
      decided: results.filter((x) => x.done).length, unauthorized: results.filter((x) => x.done?.refused?.status === 401).length,
      accepted: results.filter((x) => x.done?.ok).length, entries: indexes.length, depReads: after.length, depsLog: after,
      before: before.outcome,
    };
    put("bf15", row);
    log(`BF-15 ${how}: ${JSON.stringify({ ...row, depsLog: undefined })}`);
  }
}

function report(): void {
  summarize10("bf10");
  summarize10("bf10mv");
  for (const r of readRows("bf15")) log(`BF-15 ${r.how}: ${r.unauthorized}/${r.queued} got 401, ${r.accepted} accepted, ${r.decided} decided, ${r.entries} entries, ${r.depReads} dependency reads`);
}

const rows: Record<string, () => Promise<void> | void> = { setup, bf10, bf10mv, bf11, bf15, report };
await rows[row]!();
writeFileSync(`${RAW}${opt.run}.${row}.done`, new Date().toISOString());
