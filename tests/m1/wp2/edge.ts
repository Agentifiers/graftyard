// M1-WP2 test edge, `graft-m1-wp2-edge` on workers.dev. It binds the `Member` class of graft-m1-site-a/b/c with
// `script_name` and runs the caller's side of the write and read paths (design/DESIGN.md, "Commit and read paths"):
//
// - One RequestId per client operation, reused on every re-send of that operation's command.
// - The group lookup rule as far as M1-WP1 builds it: the cached config, else the bootstrap names built from the
//   chain's PlacementRecord; the config is then taken from `NotLeader.config`, and the leader from `ReplyMeta`.
// - `commit` waits COMMIT_HOLD + CALL_DEADLINE for its reply; an `Admitted` ticket is followed by one `outcome`.
// - A missed receipt is hedged only by `suspect` reports to the other voters, never by sending the command to a
//   second member: the next send goes to the same member unless a member answered that it leads, or a `NotLeader`
//   named another leader.
//
// Routes (bearer token): /create, /op, /status, /audit, /log, /stats, /faults, and the gate drivers /bf12, /bf13,
// /bench. Every route takes `chain` (a ChainId text form, `repo:<n>`) and `gen`.

interface Env { MEMBER_0: DurableObjectNamespace; MEMBER_1: DurableObjectNamespace; MEMBER_2: DurableObjectNamespace; TOKEN: string }

const CALL_DEADLINE = 1_000;
const COMMIT_HOLD = 1_000;
const OP_DEADLINE = 60_000;
const HINTS = ["enam", "weur", "apac"] as const;

type Op =
  | { kind: "put"; key: string; value: string }
  | { kind: "cas"; key: string; expect: string | null; value: string }
  | { kind: "append"; key: string; value: string }
  | { kind: "get"; key: string };
type Voter = { site: number; do_name: string };
type NotLeader = { term: number; leader: number | null; config: { index: number; config: Record<string, any> } | null };
type Meta = { term: number; leader: number | null; configIndex: number } | null;
type Outcome = { ok: { index: number; out: boolean; quorum: number[] } } | { refused: { index: number; err: string } } | { notLeader: NotLeader };
type Result = { ok: true; outcome: Outcome } | { ok: true; value: string | null; index: number } | { ok: false; error: string };
type Trace = string[];

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
/** Per isolate: the last config and leader each chain's replies named. */
const configCache = new Map<string, { index: number; voters: Voter[] }>();
const leaderCache = new Map<string, number>();
const termSeen = new Map<string, number>();

function record(gen: number, home = 0) {
  return { name: "Tri", sites: [0, 1, 2], home, generation: gen, moving_from: null, at: 1 };
}

/** The bootstrap names from the record: `{chain}#{generation}#f{slot}`. */
function bootstrap(chain: string, gen: number): Voter[] {
  return record(gen).sites.map((site, slot) => ({ site, do_name: `${chain}#${gen}#f${slot}` }));
}

function voters(chain: string, gen: number): Voter[] {
  return configCache.get(chain)?.voters ?? bootstrap(chain, gen);
}

/** A committed config from `NotLeader`: the newest placement's voters. */
function learnConfig(chain: string, nl: NotLeader | null | undefined): void {
  const c = nl?.config;
  if (!c) return;
  const p = c.config.normal ?? c.config.degraded?.placement ?? c.config.joint?.new;
  if (!p?.voters) return;
  if ((configCache.get(chain)?.index ?? -1) <= c.index) {
    configCache.set(chain, { index: c.index, voters: p.voters.map((v: any) => ({ site: v.site, do_name: v.do_name })) });
  }
}

function learnMeta(chain: string, meta: Meta): void {
  if (!meta) return;
  termSeen.set(chain, Math.max(termSeen.get(chain) ?? 0, meta.term));
  if (meta.leader !== null && meta.leader !== undefined) leaderCache.set(chain, meta.leader);
}

/** Packs the colo's IATA code into the integer `Colo` the member counts reports by. */
function coloId(colo: string): number {
  let n = 0;
  for (const ch of colo.slice(0, 4)) n = n * 256 + ch.charCodeAt(0);
  return n;
}

export class Edge {
  /** B0-07 (`tests/m0carried/b07-edge.ts`): told of every member call's site and whether it answered in time. */
  onCall?: (site: number, ok: boolean) => void;
  /** `home`: the record's home site (B0-07 makes groups whose home is site 2). */
  constructor(private env: Env, readonly chain: string, private gen: number, private colo: number, private home = 0) {}

  stubFor(site: number): DurableObjectStub {
    return this.stub(site);
  }

  private stub(site: number): DurableObjectStub {
    const v = voters(this.chain, this.gen).find((x) => x.site === site) ?? bootstrap(this.chain, this.gen)[site];
    const ns = this.env[`MEMBER_${site}` as "MEMBER_0"];
    return ns.get(ns.idFromName(v.do_name), { locationHint: HINTS[site] });
  }

  /** One member call with a deadline. `null` is a missed receipt or a failed call. */
  async call(site: number, path: string, body?: unknown, deadline = CALL_DEADLINE): Promise<any | null> {
    const ctl = new AbortController();
    const timer = setTimeout(() => ctl.abort(), deadline);
    try {
      const init: RequestInit = body === undefined ? { signal: ctl.signal } : { method: "POST", body: JSON.stringify(body), signal: ctl.signal };
      const res = await this.stub(site).fetch(`https://member${path}`, init);
      if (res.status !== 200) { await res.body?.cancel(); this.onCall?.(site, false); return null; }
      const v = await res.json<any>();
      this.onCall?.(site, true);
      learnMeta(this.chain, v.meta ?? null);
      return v;
    } catch {
      this.onCall?.(site, false);
      return null;
    } finally {
      clearTimeout(timer);
    }
  }

  /** The hedge: a `suspect` report about `about` to each other voter (all voters when `all`), in parallel. Returns
   *  the member that answered it leads, if one did. Carries no command. */
  async hedge(about: number, all: boolean, tr: Trace): Promise<number | null> {
    const to = voters(this.chain, this.gen).map((v) => v.site).filter((s) => all || s !== about);
    const term = termSeen.get(this.chain) ?? 0;
    const rs = await Promise.all(to.map(async (s) => [s, await this.call(s, "/suspect", { colo: this.colo, about, term })] as const));
    let leader: number | null = null;
    for (const [s, r] of rs) {
      tr.push(`suspect ${about} to ${s}: ${r?.verdict ?? "no reply"}`);
      if (r && ["leader", "won", "resumed"].includes(r.verdict)) leader = s;
    }
    if (leader !== null) leaderCache.set(this.chain, leader);
    return leader;
  }

  /** `NotLeader` from `target`: follow a named leader, else report to every voter and move to the next one. */
  private async notLeader(nl: NotLeader, target: number, tries: number, tr: Trace): Promise<number> {
    learnConfig(this.chain, nl);
    termSeen.set(this.chain, Math.max(termSeen.get(this.chain) ?? 0, nl.term));
    if (nl.leader !== null && nl.leader !== target) { leaderCache.set(this.chain, nl.leader); return nl.leader; }
    leaderCache.delete(this.chain);
    const led = await this.hedge(target, true, tr);
    if (led !== null) return led;
    if (tries % 3 === 0) await sleep(200);
    const vs = voters(this.chain, this.gen).map((v) => v.site);
    return vs[(vs.indexOf(target) + 1) % vs.length];
  }

  /** One client write to completion through whichever member leads, re-sent under `rid` until it has an outcome.
   *  `kill` rides only on sends until a leader admitted the command or a send failed, so a fault fires once. */
  async write(op: Exclude<Op, { kind: "get" }>, rid: string, tr: Trace, kill?: { mode: string; down_ms?: number }): Promise<Result & { sends: number }> {
    const end = Date.now() + OP_DEADLINE;
    let target = leaderCache.get(this.chain) ?? this.home;
    let sends = 0, tries = 0;
    const cmd = op;
    while (Date.now() < end) {
      sends++;
      const c = await this.call(target, "/commit", { rid, cmd, ...(kill ? { kill: kill.mode, down_ms: kill.down_ms } : {}) }, COMMIT_HOLD + CALL_DEADLINE);
      if (c === null) {
        kill = undefined;
        tr.push(`commit to ${target}: missed`);
        const led = await this.hedge(target, false, tr);
        if (led !== null) target = led;
        else await sleep(Math.min(1_000, 100 * 2 ** Math.min(sends, 4)));
        continue;
      }
      if (c.error?.notLeader) {
        tr.push(`commit to ${target}: notLeader ${JSON.stringify(c.error.notLeader.leader)}`);
        target = await this.notLeader(c.error.notLeader, target, ++tries, tr);
        continue;
      }
      if (c.error) return { ok: false, error: JSON.stringify(c.error), sends };
      kill = undefined;
      let done: Outcome | undefined = c.done;
      if (c.admitted) {
        tr.push(`commit to ${target}: admitted`);
        const o = await this.call(target, "/outcome", { ticket: c.admitted, wait_ms: 10_000 }, 11_000);
        if (o === null) {
          tr.push(`outcome at ${target}: missed`);
          const led = await this.hedge(target, false, tr);
          if (led !== null) target = led;
          continue;
        }
        done = o.done;
        if (!done) continue; // still pending past the wait: re-send under the same id
      }
      if (done && "notLeader" in done) {
        tr.push(`outcome at ${target}: notLeader`);
        target = await this.notLeader(done.notLeader, target, ++tries, tr);
        continue;
      }
      leaderCache.set(this.chain, target);
      return { ok: true, outcome: done!, sends };
    }
    return { ok: false, error: `gave up after ${OP_DEADLINE} ms`, sends };
  }

  /** One `fresh` read through the leader. */
  async read(key: string, tr: Trace): Promise<Result & { sends: number }> {
    const end = Date.now() + OP_DEADLINE;
    let target = leaderCache.get(this.chain) ?? this.home;
    let sends = 0, tries = 0;
    while (Date.now() < end) {
      sends++;
      const r = await this.call(target, "/read", { key }, 3 * CALL_DEADLINE + 500);
      if (r === null) {
        tr.push(`read at ${target}: missed`);
        const led = await this.hedge(target, false, tr);
        if (led !== null) target = led;
        await sleep(100);
        continue;
      }
      if (r.error?.notLeader) { target = await this.notLeader(r.error.notLeader, target, ++tries, tr); continue; }
      if (r.busy) { await sleep(50); continue; }
      if (r.error) return { ok: false, error: JSON.stringify(r.error), sends };
      leaderCache.set(this.chain, target);
      return { ok: true, value: r.value ?? null, index: r.index, sends };
    }
    return { ok: false, error: `gave up after ${OP_DEADLINE} ms`, sends };
  }

  op(op: Op, rid: string, tr: Trace) {
    return op.kind === "get" ? this.read(op.key, tr) : this.write(op, rid, tr);
  }

  all<T = any>(path: string, deadline = 10_000): Promise<{ site: number; r: T | null }[]> {
    return Promise.all([0, 1, 2].map(async (site) => ({ site, r: await this.call(site, path, undefined, deadline) as T | null })));
  }

  /** Joins the three bootstrap members, then makes the creating call on the record's home, which proposes the
   *  bootstrap entry and campaigns for term 2 once a quorum stored it. */
  async create(nearMs?: number): Promise<unknown> {
    const rec = record(this.gen, this.home);
    const joins = await Promise.all(rec.sites.map(async (site, slot) => {
      const me = { site, kind: "full", slot, generation: this.gen, do_name: `${this.chain}#${this.gen}#f${slot}` };
      const res = await this.stub(site).fetch("https://member/join", { method: "POST", body: JSON.stringify({ chain: this.chain, me, record: rec }) });
      return { site, status: res.status, body: await res.json().catch(() => null) };
    }));
    const tr: Trace = [];
    const t0 = Date.now();
    let verdict = null;
    for (let i = 0; i < 50; i++) {
      verdict = (await this.call(rec.home, "/campaign", {}))?.verdict ?? null;
      const st = await this.call(rec.home, "/status");
      if (st?.role === "leader") { leaderCache.set(this.chain, rec.home); break; }
      await sleep(200);
    }
    // The near check, as `graftyard_lookup::create` runs it: the leader re-places a voter until the home has one
    // within `NEAR_RTT`, or keeps the nearest try. `nearMs` bounds every region at that instead (test installs).
    const placed = await this.call(rec.home, "/place", nearMs === undefined ? {} : { near_ms: nearMs }, 300_000);
    if (!placed || placed.error) return { joins, verdict, leaderMs: Date.now() - t0, placed, warm: { ok: false }, trace: tr };
    await this.learnVoters();
    const w = await this.write({ kind: "put", key: "warm", value: "1" }, rid(), tr);
    return { joins, verdict, leaderMs: Date.now() - t0, placed, warm: w, trace: tr };
  }

  /** The home member's committed config, so calls by site reach a re-placed voter's new object. The home member is
   *  never re-placed: `/place` replaces only the home's followers. */
  async learnVoters(): Promise<void> {
    const st = await this.call(record(this.gen, this.home).home, "/status");
    if (st?.config) learnConfig(this.chain, { config: st.config } as NotLeader);
  }
}

/** A `RequestId`: a ULID, 48 bits of milliseconds then 80 random bits, as 26 Crockford base-32 characters. */
export function rid(): string {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = BigInt(Date.now()) & 0xffff_ffff_ffffn;
  for (const b of crypto.getRandomValues(new Uint8Array(10))) v = (v << 8n) | BigInt(b);
  let out = "";
  for (let i = 25; i >= 0; i--) out += A[Number((v >> BigInt(5 * i)) & 31n)];
  return out;
}

/** Two outcomes are the same when they name the same index and the same output or refusal. The quorum a replay
 *  carries is the leader's latest commit proof, which may name other sites than the original's (`m1-wp2.md`). */
function same(a: Outcome | null, b: Outcome | null): boolean {
  const key = (o: Outcome | null) => o === null ? null
    : "ok" in o ? ["ok", o.ok.index, o.ok.out] : "refused" in o ? ["refused", o.refused.index, o.refused.err] : ["notLeader"];
  return a !== null && b !== null && JSON.stringify(key(a)) === JSON.stringify(key(b));
}

/** BF-12: `n` sequential kills on one group. Each appends a unique token with a fresh request id and a kill between
 *  quorum and reply; the edge re-sends under the same id until it has an outcome, which is compared with the original
 *  the killed leader recorded in its `fault` row before it reset. */
async function bf12(e: Edge, key: string, from: number, plan: { mode: string; down_ms?: number }[]) {
  const kills = [];
  for (let i = 0; i < plan.length; i++) {
    const id = rid(), token = `${from + i},`, tr: Trace = [];
    const t0 = Date.now();
    const r = await e.write({ kind: "append", key, value: token }, id, tr, plan[i]);
    const ms = Date.now() - t0;
    const faults = await e.all<any[]>(`/faults?rid=${id}`);
    const rows = faults.flatMap((f) => (f.r ?? []).map((x: any) => ({ site: f.site, ...x })));
    const original = rows[0]?.result ?? null;
    const outcome = r.ok && "outcome" in r ? r.outcome : null;
    kills.push({
      rid: id, token, mode: plan[i].mode, down_ms: plan[i].down_ms ?? 0, ms, sends: r.sends, landed: rows.length, killedSite: rows[0]?.site ?? null,
      original, outcome, error: r.ok ? null : r.error, same: same(original, outcome),
      trace: rows.length && same(original, outcome) ? undefined : tr,
    });
  }
  return kills;
}

/** BF-13: one round on key `k`. `hold` puts an entry in flight; `cas1` (expect unset) and `cas2` (expect unset) are
 *  admitted behind it in one turn, so they share the next entry and `cas2` is decided Stale only because of `cas1`.
 *  The leader is killed before that entry commits (`mode`). The edge re-sends `hold` and `cas2` under their ids;
 *  `cas1`'s client gives up, so `cas2` may only be refused if `cas1` committed. Then a read of `k`. Returns history
 *  rows in the M0 checker's form, with client ids from `client0`. */
async function bf13(e: Edge, k: string, client0: number, mode: string, down_ms: number) {
  const ids = { hold: rid(), cas1: rid(), cas2: rid() };
  const ops = {
    hold: { kind: "put", key: `${k}:hold`, value: "h" } as const,
    cas1: { kind: "cas", key: k, expect: null, value: "a" } as const,
    cas2: { kind: "cas", key: k, expect: null, value: "b" } as const,
  };
  const tr: Trace = [];
  // Find the leader first, so the batch reaches it.
  const warm = await e.read(`${k}:warm`, tr);
  const leader = leaderCache.get(e.chain) ?? 0;
  const call = Date.now();
  const batch = await e.call(leader, "/commit_many", { cmds: [
    { rid: ids.hold, cmd: ops.hold },
    { rid: ids.cas1, cmd: ops.cas1, kill: mode, down_ms },
    { rid: ids.cas2, cmd: ops.cas2 },
  ] }, 3 * COMMIT_HOLD + CALL_DEADLINE);
  const history: any[] = [];
  const results: Record<string, any> = {};
  const outRow = (name: "hold" | "cas1" | "cas2", client: number, ret: number, o: Outcome | null) => {
    results[name] = o;
    if (o === null) { history.push({ client, call, ret, op: ops[name], ok: false }); return; }
    const ok = "ok" in o;
    history.push({ client, call, ret, op: ops[name], ok: true, out: { ok } });
  };
  if (batch?.results && batch.results.every((x: any) => x.done && !x.done.notLeader)) {
    // The kill did not fire (the leader moved first): every command has its outcome.
    const ret = Date.now();
    outRow("hold", client0, ret, batch.results[0].done);
    outRow("cas1", client0 + 1, ret, batch.results[1].done);
    outRow("cas2", client0 + 2, ret, batch.results[2].done);
  } else {
    tr.push(`batch: ${batch === null ? "missed" : JSON.stringify(batch.results).slice(0, 300)}`);
    const [h, c2] = await Promise.all([
      e.write(ops.hold, ids.hold, tr),
      e.write(ops.cas2, ids.cas2, tr),
    ]);
    const ret = Date.now();
    outRow("hold", client0, ret, h.ok && "outcome" in h ? h.outcome : null);
    outRow("cas2", client0 + 2, ret, c2.ok && "outcome" in c2 ? c2.outcome : null);
    outRow("cas1", client0 + 1, ret, null); // the client gave up: its effect is unknown
  }
  const gcall = Date.now();
  const g = await e.read(k, tr);
  history.push(g.ok && "value" in g
    ? { client: client0 + 3, call: gcall, ret: Date.now(), op: { kind: "get", key: k }, ok: true, out: { value: g.value } }
    : { client: client0 + 3, call: gcall, ret: Date.now(), op: { kind: "get", key: k }, ok: false });
  const faults = await e.all<any[]>(`/faults?rid=${ids.cas1}`);
  const rows = faults.flatMap((f) => (f.r ?? []).map((x: any) => ({ site: f.site, ...x })));
  return { k, ids, leader, warm: warm.ok, results, read: g, fault: rows[0] ?? null, landed: rows.length, history, trace: tr };
}

/** B0-01-style load from inside the edge: `n` puts of `bytes` bytes, `conc` workers, each paced at `rate` per s. */
async function bench(e: Edge, n: number, conc: number, rate: number, bytes: number) {
  const value = "x".repeat(bytes);
  const lat: number[] = [];
  const errors: Record<string, number> = {};
  let i = 0, failed = 0;
  const t0 = Date.now();
  await Promise.all(Array.from({ length: conc }, async () => {
    if (rate) await sleep(Math.random() * 1000 / rate);
    while (i < n) {
      const k = i++;
      const t = Date.now();
      const r = await e.write({ kind: "put", key: `bench${k % 64}`, value }, rid(), []);
      if (r.ok) lat.push(Date.now() - t); else { failed++; errors[r.error] = (errors[r.error] ?? 0) + 1; }
      if (rate) await sleep(Math.max(0, 1000 / rate - (Date.now() - t)));
    }
  }));
  return { n, failed, wallMs: Date.now() - t0, lat, errors };
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url);
    const body: Record<string, any> = req.method === "POST" ? await req.json() : {};
    const chain = String(body.chain ?? url.searchParams.get("chain") ?? "repo:1");
    const gen = Number(body.gen ?? url.searchParams.get("gen") ?? 1);
    const colo = (req.cf as { colo?: string } | undefined)?.colo ?? "edge";
    const e = new Edge(env, chain, gen, coloId(String(body.colo ?? colo)));
    const json = (v: unknown, status = 200) => Response.json(v, { status });
    try {
      switch (url.pathname) {
        case "/create": return json(await e.create(body.nearMs));
        case "/op": {
          const tr: Trace = [];
          const r = body.op.kind === "get" ? await e.read(body.op.key, tr) : await e.write(body.op, body.rid ?? rid(), tr, body.kill);
          return json({ ...r, trace: tr });
        }
        case "/status": await e.learnVoters(); return json(await e.all("/status"));
        case "/raw": {
          await e.learnVoters();
          // One member's raw answer to a GET, for diagnosis.
          const site = Number(url.searchParams.get("site") ?? 0);
          const res = await e.stubFor(site).fetch(`https://member${url.searchParams.get("path") ?? "/status"}`);
          return json({ status: res.status, body: (await res.text()).slice(0, 4000) });
        }
        case "/audit": return json(await e.all(`/audit?prefix=${encodeURIComponent(url.searchParams.get("prefix") ?? "")}`, 20_000));
        case "/log": return json(await e.all(`/log?from=${url.searchParams.get("from") ?? 1}&n=${url.searchParams.get("n") ?? 500}`, 20_000));
        case "/faults": return json(await e.all("/faults", 20_000));
        case "/stats": {
          await e.learnVoters();
          // B0-02's A/Bs (`tests/m0/results/carried-5.md`, `carried-6.md`): `body` "array" or "stream" sets how each member
          // sends calls; `arrival` "mem" or "sql" where BF-14's arrival rows go; `entry` "max" or "soft" where the leader
          // stops filling an entry (`carried-7.md`).
          const q = [body.reset ? "reset=1" : "", body.body ? `body=${body.body}` : "", body.arrival ? `arrival=${body.arrival}` : "", body.entry ? `entry=${body.entry}` : ""].filter(Boolean).join("&");
          return json(await e.all(`/stats${q ? `?${q}` : ""}`, 20_000));
        }
        case "/bf12": return json({ colo, kills: await bf12(e, body.key, body.from ?? 0, body.plan ?? [{ mode: "after_quorum" }]) });
        case "/bf13": return json({ colo, ...(await bf13(e, body.key, body.client0 ?? 0, body.mode ?? "before_commit", body.down_ms ?? 0)) });
        case "/bench": return json({ colo, ...(await bench(e, body.n ?? 100, body.concurrency ?? 5, body.ratePerWorker ?? 0, body.bytes ?? 1024)) });
      }
    } catch (err) {
      return json({ error: String(err) }, 500);
    }
    return json({ error: "not found" }, 404);
  },
};
