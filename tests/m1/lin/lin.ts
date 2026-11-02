// graft-m1-lin-tools: the 24-hour linearizability rerun's load and chaos (BM-1, "Linearizability rerun at 24
// hours"; B0-11's carried note), on its own install graft-m1-lin-a/b/c built with the `test-faults` feature. It binds
// the three sites' `Member` classes with `script_name` and holds two Durable Object classes:
//
// - `LoadClient`, pinned to one colo (a Durable Object never moves once created). Each runs `concurrency` logical
//   clients in 20 s alarm chunks, 40% `get` and the rest `put` and `cas` over `keys` registers, through the edge rules
//   of M1-WP2: one RequestId per operation, re-sent under the same id; a missed receipt hedged only by `suspect` to the
//   other voters; the config from `NotLeader` and the leader from `ReplyMeta`. A client gives up after 30 s and records
//   the operation with no reply. Values carry the chunk's start, so every write is unique in the run. At each chunk's
//   start and end it reads every member's clock (`/status` `now`) and records the offset against the midpoint of the
//   call, with the call's round trip as its bound.
// - `Chaos`, B0-11's chaos loop as an alarm every 8 to 16 s: 60% a member reset (half on the leader), else a transfer
//   from the leader to one of the other sites, 30% of them abandoned (their `TimeoutNow` is dropped, so the leader gives up
//   after the LEASE). Before each step it reads every member's per-isolate fault counters (`/isolate`) and keeps the
//   latest per isolate. The call faults themselves (5% failed member calls, 5% lost AppendEntries replies) are the
//   sites' `GRAFTYARD_FAULTS`, set once at deploy.
//
// Routes (bearer token): POST /create {chain}, /clients/place {names, hint}, /clients/start {names, spec},
// /clients/stop {names}, /clients/status {names}, /clients/history {name, run, after}, /clients/clock {name, after},
// /chaos/start {run, chain, end}, /chaos/stop {run}, /chaos/events {run, after}, /chaos/isolates {run},
// /status {chain}, /member {chain, site, path, body?}.

import { DurableObject } from "cloudflare:workers";

interface Env {
  MEMBER_0: DurableObjectNamespace; MEMBER_1: DurableObjectNamespace; MEMBER_2: DurableObjectNamespace;
  CLIENT: DurableObjectNamespace<LoadClient>; CHAOS: DurableObjectNamespace<Chaos>; TOKEN: string;
}

const CALL_DEADLINE = 1_000;
const COMMIT_HOLD = 1_000;
const GIVE_UP = 30_000;
const CHUNK_MS = 20_000;
const HISTORY_PAGE = 5_000;
const HINTS = ["enam", "weur", "apac"] as const;
const SITES = [0, 1, 2];

type Op = { kind: "put"; key: string; value: string } | { kind: "cas"; key: string; expect: string | null; value: string } | { kind: "get"; key: string };
type Out = { value?: string | null; ok?: boolean };
type Voter = { site: number; do_name: string };
export interface LoadSpec { run: string; chain: string; concurrency: number; rate: number; end: number; keys: number; readShare: number; clientBase: number }

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function record() {
  return { name: "Tri", sites: SITES, home: 0, generation: 1, moving_from: null, at: 1 };
}

function bootstrap(chain: string): Voter[] {
  return SITES.map((site, slot) => ({ site, do_name: `${chain}#1#f${slot}` }));
}

async function colo(): Promise<string> {
  const t = await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text();
  return /^colo=(\w+)$/m.exec(t)?.[1] ?? "unknown";
}

function coloId(c: string): number {
  let n = 0;
  for (const ch of c.slice(0, 4)) n = n * 256 + ch.charCodeAt(0);
  return n;
}

/** A RequestId: a ULID as 26 Crockford base-32 characters. */
function rid(): string {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = BigInt(Date.now()) & 0xffff_ffff_ffffn;
  for (const b of crypto.getRandomValues(new Uint8Array(10))) v = (v << 8n) | BigInt(b);
  let out = "";
  for (let i = 25; i >= 0; i--) out += A[Number((v >> BigInt(5 * i)) & 31n)];
  return out;
}

/** The caller's side of the commit and read paths for one chain, shared by the logical clients of one object. */
class Edge {
  private voters: Voter[];
  private configIndex = -1;
  leader: number | null = null;
  private term = 0;
  constructor(private env: Env, readonly chain: string, private colo: number) { this.voters = bootstrap(chain); }

  private stub(site: number): DurableObjectStub {
    const v = this.voters.find((x) => x.site === site) ?? bootstrap(this.chain)[site];
    const ns = this.env[`MEMBER_${site}` as "MEMBER_0"];
    return ns.get(ns.idFromName(v.do_name), { locationHint: HINTS[site] });
  }

  /** One member call with a deadline; `null` is a failed call or a missed receipt. A new stub every call: a stub
   *  that threw stays broken after its object resets. */
  async call(site: number, path: string, body?: unknown, deadline = CALL_DEADLINE): Promise<any | null> {
    try {
      const init: RequestInit = body === undefined ? {} : { method: "POST", body: JSON.stringify(body) };
      const res = await this.stub(site).fetch(`https://member${path}`, { ...init, signal: AbortSignal.timeout(deadline) });
      if (res.status !== 200) { await res.body?.cancel(); return null; }
      const v = await res.json<any>();
      if (v?.meta) {
        this.term = Math.max(this.term, v.meta.term ?? 0);
        if (v.meta.leader !== null && v.meta.leader !== undefined) this.leader = v.meta.leader;
      }
      return v;
    } catch {
      return null;
    }
  }

  private learn(nl: any): void {
    this.term = Math.max(this.term, nl?.term ?? 0);
    const c = nl?.config;
    const p = c?.config?.normal ?? c?.config?.degraded?.placement ?? c?.config?.joint?.new;
    if (p?.voters && c.index >= this.configIndex) {
      this.configIndex = c.index;
      this.voters = p.voters.map((v: any) => ({ site: v.site, do_name: v.do_name }));
    }
  }

  /** `suspect` about `about` to the other voters (every voter when `all`); the member that answers it leads. */
  private async hedge(about: number, all: boolean): Promise<number | null> {
    const to = this.voters.map((v) => v.site).filter((s) => all || s !== about);
    const rs = await Promise.all(to.map(async (s) => [s, await this.call(s, "/suspect", { colo: this.colo, about, term: this.term })] as const));
    for (const [s, r] of rs) if (r && ["leader", "won", "resumed"].includes(r.verdict)) { this.leader = s; return s; }
    return null;
  }

  private async notLeader(nl: any, target: number, tries: number): Promise<number> {
    this.learn(nl);
    if (nl?.leader !== null && nl?.leader !== undefined && nl.leader !== target) { this.leader = nl.leader; return nl.leader; }
    this.leader = null;
    const led = await this.hedge(target, true);
    if (led !== null) return led;
    if (tries % 3 === 0) await sleep(200);
    const vs = this.voters.map((v) => v.site);
    return vs[(vs.indexOf(target) + 1) % vs.length];
  }

  /** A write to completion under one RequestId, or `null` after GIVE_UP. */
  async write(cmd: Exclude<Op, { kind: "get" }>, id: string): Promise<{ ok: boolean } | null> {
    const end = Date.now() + GIVE_UP;
    let target = this.leader ?? 0, sends = 0, tries = 0;
    const body = { rid: id, cmd };
    while (Date.now() < end) {
      sends++;
      const c = await this.call(target, "/commit", body, COMMIT_HOLD + CALL_DEADLINE);
      if (c === null) {
        const led = await this.hedge(target, false);
        if (led !== null) target = led; else await sleep(Math.min(1_000, 100 * 2 ** Math.min(sends, 4)));
        continue;
      }
      if (c.error?.notLeader) { target = await this.notLeader(c.error.notLeader, target, ++tries); continue; }
      if (c.error) { await sleep(200); continue; }
      let done = c.done;
      if (c.admitted) {
        const o = await this.call(target, "/outcome", { ticket: c.admitted, wait_ms: Math.max(0, Math.min(10_000, end - Date.now())) }, 11_000);
        if (o === null) { const led = await this.hedge(target, false); if (led !== null) target = led; continue; }
        done = o.done;
        if (!done) continue;
      }
      if (done?.notLeader) { target = await this.notLeader(done.notLeader, target, ++tries); continue; }
      this.leader = target;
      if (done?.ok) return { ok: true };
      if (done?.refused) return { ok: false };
    }
    return null;
  }

  /** A `fresh` read through the leader, or `null` after GIVE_UP. */
  async read(key: string): Promise<{ value: string | null } | null> {
    const end = Date.now() + GIVE_UP;
    let target = this.leader ?? 0, tries = 0;
    while (Date.now() < end) {
      const r = await this.call(target, "/read", { key }, 3 * CALL_DEADLINE + 500);
      if (r === null) { const led = await this.hedge(target, false); if (led !== null) target = led; await sleep(100); continue; }
      if (r.error?.notLeader) { target = await this.notLeader(r.error.notLeader, target, ++tries); continue; }
      if (r.busy || r.error) { await sleep(50); continue; }
      this.leader = target;
      return { value: r.value ?? null };
    }
    return null;
  }

  /** Each member's clock against this object's: offset = member `now` - midpoint of the call, bound = round trip. */
  async clocks(): Promise<{ site: number; offset: number | null; rtt: number }[]> {
    return Promise.all(SITES.map(async (site) => {
      const t0 = Date.now();
      const st = await this.call(site, "/status", undefined, 5_000);
      const t1 = Date.now();
      return { site, offset: typeof st?.now === "number" ? st.now - (t0 + t1) / 2 : null, rtt: t1 - t0 };
    }));
  }
}

export class LoadClient extends DurableObject<Env> {
  private sql = this.ctx.storage.sql;
  private lastRead = new Map<string, string | null>();
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql.exec("CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL)");
    this.sql.exec(`CREATE TABLE IF NOT EXISTS history (seq INTEGER PRIMARY KEY AUTOINCREMENT, run TEXT NOT NULL, client INTEGER NOT NULL,
      call INTEGER NOT NULL, ret INTEGER NOT NULL, op TEXT NOT NULL, ok INTEGER NOT NULL, out TEXT, rid TEXT)`);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS clock (seq INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, phase TEXT NOT NULL,
      site INTEGER NOT NULL, offset REAL, rtt INTEGER NOT NULL)`);
  }
  private meta(k: string) { return this.sql.exec<{ v: string }>("SELECT v FROM meta WHERE k = ?", k).toArray()[0]?.v; }
  private setMeta(k: string, v: string) { this.sql.exec("INSERT OR REPLACE INTO meta VALUES (?, ?)", k, v); }

  async place(): Promise<string> {
    let c = this.meta("colo");
    if (!c) { c = await colo(); this.setMeta("colo", c); }
    return c;
  }

  async start(spec: LoadSpec): Promise<void> {
    await this.place();
    this.setMeta("spec", JSON.stringify(spec));
    await this.ctx.storage.setAlarm(Date.now());
  }

  async stop(): Promise<void> {
    const spec = this.meta("spec");
    if (spec) this.setMeta("spec", JSON.stringify({ ...JSON.parse(spec), end: 0 }));
    await this.ctx.storage.deleteAlarm();
  }

  private async clocks(e: Edge, phase: string): Promise<void> {
    const at = Date.now();
    for (const c of await e.clocks()) this.sql.exec("INSERT INTO clock (at, phase, site, offset, rtt) VALUES (?, ?, ?, ?, ?)", at, phase, c.site, c.offset, c.rtt);
  }

  async alarm(): Promise<void> {
    const spec = JSON.parse(this.meta("spec")!) as LoadSpec;
    if (Date.now() >= spec.end) return;
    const chunkEnd = Math.min(spec.end, Date.now() + CHUNK_MS);
    const e = new Edge(this.env, spec.chain, coloId(this.meta("colo") ?? "edge"));
    await this.clocks(e, "start");
    const chunk = Date.now().toString(36);
    let n = 0;
    const gap = 1000 / spec.rate;
    const loop = async (client: number) => {
      while (Date.now() < chunkEnd) {
        const t0 = Date.now();
        const op = this.pick(spec, client, `${chunk}-${n++}`);
        const call = Date.now();
        let out: Out | null;
        const id = op.kind === "get" ? null : rid();
        try {
          if (op.kind === "get") {
            const r = await e.read(op.key);
            out = r && { value: r.value };
            if (r) this.lastRead.set(`${client}/${op.key}`, r.value);
          } else {
            const r = await e.write(op, id!);
            out = r && { ok: r.ok };
          }
        } catch {
          out = null;
        }
        this.sql.exec("INSERT INTO history (run, client, call, ret, op, ok, out, rid) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
          spec.run, client, call, Date.now(), JSON.stringify(op), out ? 1 : 0, out ? JSON.stringify(out) : null, id);
        const wait = gap - (Date.now() - t0);
        if (wait > 0) await sleep(wait);
      }
    };
    await Promise.all(Array.from({ length: spec.concurrency }, (_, k) => loop(spec.clientBase + k)));
    await this.clocks(e, "end");
    if (Date.now() < spec.end) await this.ctx.storage.setAlarm(Date.now());
  }

  private pick(spec: LoadSpec, client: number, i: string): Op {
    const key = `k${Math.floor(Math.random() * spec.keys)}`;
    const value = `${client}.${i}`;
    const r = Math.random();
    if (r < spec.readShare) return { kind: "get", key };
    if (r < spec.readShare + (1 - spec.readShare) / 2) return { kind: "put", key, value };
    return { kind: "cas", key, expect: this.lastRead.get(`${client}/${key}`) ?? null, value };
  }

  async status(): Promise<object> {
    const r = this.sql.exec<{ n: number; f: number; last: number | null }>("SELECT count(*) AS n, coalesce(sum(1 - ok), 0) AS f, max(ret) AS last FROM history").one();
    const spec = this.meta("spec");
    return { colo: this.meta("colo") ?? "", rows: r.n, failed: r.f, lastRet: r.last, end: spec ? JSON.parse(spec).end : 0,
      running: (await this.ctx.storage.getAlarm()) !== null };
  }

  async history(run: string, after: number): Promise<object[]> {
    const c = this.meta("colo") ?? "";
    return this.sql.exec<any>("SELECT seq, client, call, ret, op, ok, out, rid FROM history WHERE run = ? AND seq > ? ORDER BY seq LIMIT ?",
      run, after, HISTORY_PAGE).toArray().map((r) => ({ seq: r.seq, client: r.client, colo: c, call: r.call, ret: r.ret,
      op: JSON.parse(r.op), ok: r.ok === 1, ...(r.out ? { out: JSON.parse(r.out) } : {}), ...(r.rid ? { rid: r.rid } : {}) }));
  }

  async clock(after: number): Promise<object[]> {
    return this.sql.exec("SELECT seq, at, phase, site, offset, rtt FROM clock WHERE seq > ? ORDER BY seq LIMIT ?", after, HISTORY_PAGE).toArray();
  }
}

type ChaosCfg = { run: string; chain: string; end: number };

export class Chaos extends DurableObject<Env> {
  private sql = this.ctx.storage.sql;
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql.exec("CREATE TABLE IF NOT EXISTS ev (seq INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER, what TEXT, result TEXT)");
    this.sql.exec("CREATE TABLE IF NOT EXISTS isolate (site INTEGER, isolate TEXT, at INTEGER, row TEXT, PRIMARY KEY (site, isolate))");
  }
  private log(what: string, result: string): void { this.sql.exec("INSERT INTO ev (at, what, result) VALUES (?, ?, ?)", Date.now(), what, result); }

  async start(cfg: ChaosCfg): Promise<object> {
    await this.ctx.storage.put("cfg", cfg);
    this.log("chaos started", new Date(cfg.end).toISOString());
    await this.ctx.storage.setAlarm(Date.now() + 1_000);
    return { started: true };
  }

  async stop(): Promise<object> {
    await this.ctx.storage.deleteAlarm();
    await this.ctx.storage.delete("cfg");
    await this.poll(await this.cfgOr());
    this.log("chaos stopped", "ok");
    return { stopped: true };
  }

  private async cfgOr(): Promise<ChaosCfg | undefined> { return this.ctx.storage.get<ChaosCfg>("lastCfg"); }

  /** Every member's per-isolate counters; the latest row per (site, isolate) is kept. */
  private async poll(cfg: ChaosCfg | undefined): Promise<void> {
    if (!cfg) return;
    const e = new Edge(this.env, cfg.chain, 0);
    await Promise.all(SITES.map(async (s) => {
      const r = await e.call(s, "/isolate", undefined, 5_000);
      if (r?.isolate) this.sql.exec("INSERT OR REPLACE INTO isolate (site, isolate, at, row) VALUES (?, ?, ?, ?)", s, r.isolate, Date.now(), JSON.stringify(r));
    }));
  }

  async events(after: number): Promise<object> {
    const rows = this.sql.exec("SELECT seq, at, what, result FROM ev WHERE seq > ? ORDER BY seq LIMIT 2000", after).toArray();
    return { running: (await this.ctx.storage.getAlarm()) !== null, rows };
  }

  async isolates(): Promise<object[]> {
    return this.sql.exec<{ site: number; isolate: string; at: number; row: string }>("SELECT site, isolate, at, row FROM isolate ORDER BY site, isolate")
      .toArray().map((r) => ({ site: r.site, at: r.at, ...JSON.parse(r.row) }));
  }

  async alarm(): Promise<void> {
    const cfg = await this.ctx.storage.get<ChaosCfg>("cfg");
    if (!cfg) return;
    await this.ctx.storage.put("lastCfg", cfg);
    await this.poll(cfg);
    if (Date.now() >= cfg.end) { this.log("chaos ended", "ok"); await this.ctx.storage.delete("cfg"); return; }
    const e = new Edge(this.env, cfg.chain, 0);
    try {
      const st = await Promise.all(SITES.map((s) => e.call(s, "/status", undefined, 5_000)));
      const leader = st.findIndex((x) => x?.role === "leader");
      const terms = st.map((x) => x?.term ?? null).join("/");
      if (Math.random() < 0.6) {
        const s = leader >= 0 && Math.random() < 0.5 ? leader : Math.floor(Math.random() * 3);
        const r = await e.call(s, "/reset", {}, 5_000);
        this.log(`reset ${s}${s === leader ? " (leader)" : ""}`, `terms ${terms} ${r === null ? "reset" : JSON.stringify(r).slice(0, 80)}`);
      } else {
        // To one of the other two sites. A target inside RESET_QUARANTINE refuses, as the protocol says it must.
        const to = leader < 0 ? Math.floor(Math.random() * 3) : (leader + 1 + Math.floor(Math.random() * 2)) % 3;
        const abandon = Math.random() < 0.3;
        if (leader < 0) this.log(`transfer ->${to}${abandon ? " (abandon)" : ""}`, `no leader; terms ${terms}`);
        else {
          const r = await e.call(leader, "/transfer", { to, abandon }, 5_000);
          this.log(`transfer ${leader}->${to}${abandon ? " (abandon)" : ""}`, JSON.stringify(r?.transfer ?? r).slice(0, 120));
        }
      }
    } catch (err) {
      this.log("chaos step", String(err).slice(0, 120));
    }
    await this.ctx.storage.setAlarm(Date.now() + 8_000 * (1 + Math.random()));
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.TOKEN}`) return new Response("unauthorized", { status: 401 });
    const path = new URL(req.url).pathname;
    const body: Record<string, any> = req.method === "POST" ? await req.json() : {};
    const json = (v: unknown, status = 200) => Response.json(v, { status });
    const names = (body.names ?? []) as string[];
    const all = <T>(xs: Promise<T>[]) => Promise.allSettled(xs).then((rs) => rs.map((r) => (r.status === "fulfilled" ? r.value : { error: String(r.reason) })));
    const client = (n: string) => env.CLIENT.getByName(n);
    const chaos = () => env.CHAOS.getByName(String(body.run));
    const e = new Edge(env, String(body.chain ?? "repo:1"), 0);
    switch (path) {
      case "/create": {
        const rec = record();
        const joins = await Promise.all(SITES.map(async (site, slot) => {
          const me = { site, kind: "full", slot, generation: 1, do_name: `${e.chain}#1#f${slot}` };
          return { site, r: await e.call(site, "/join", { chain: e.chain, me, record: rec }, 30_000) };
        }));
        let verdict = null;
        for (let i = 0; i < 50; i++) {
          verdict = (await e.call(0, "/campaign", {}))?.verdict ?? null;
          if ((await e.call(0, "/status"))?.role === "leader") break;
          await sleep(200);
        }
        return json({ joins, verdict, warm: await e.write({ kind: "put", key: "warm", value: "0" }, rid()) });
      }
      case "/clients/place": return json(await all(names.map((n) => env.CLIENT.get(env.CLIENT.idFromName(n), { locationHint: body.hint }).place())));
      case "/clients/start": return json(await all(names.map((n, i) => client(n).start({ ...(body.spec as LoadSpec), clientBase: (body.base ?? 0) + i * 100 }))));
      case "/clients/stop": return json(await all(names.map((n) => client(n).stop())));
      case "/clients/status": return json(await all(names.map((n) => client(n).status())));
      case "/clients/history": return json(await client(body.name).history(body.run, body.after ?? 0));
      case "/clients/clock": return json(await client(body.name).clock(body.after ?? 0));
      case "/chaos/start": return json(await chaos().start({ run: body.run, chain: body.chain, end: body.end }));
      case "/chaos/stop": return json(await chaos().stop());
      case "/chaos/events": return json(await chaos().events(body.after ?? 0));
      case "/chaos/isolates": return json(await chaos().isolates());
      case "/status": return json(await Promise.all(SITES.map((s) => e.call(s, "/status", undefined, 5_000))));
      case "/member": return json(await e.call(Number(body.site), String(body.path), body.body, Number(body.deadline ?? 20_000)));
    }
    return json({ error: "not found" }, 404);
  },
} satisfies ExportedHandler<Env>;
