// graft-m0-tools: M0-WP1's test tools in their own script, so building or changing them never creates a site-script
// version (build/BUILD-PLAN.md, M0-WP1 "Build"). It holds three things:
//   - `FaultControl`, the run's fault plan, which members read through src/faults.ts (B0-03, B0-11, B0-25).
//   - `LoadClient`, a load client pinned to one colo. A Durable Object never moves after it is created, so a client
//     created until one lands in the wanted colo stays there. Each records a history for the Porcupine checker
//     (porcupine/), one row per operation with its call and return times.
//   - `QuorumProbe`, M0-WP11's write-confirmation timing (decisions/OPEN-QUESTIONS.md, C1).
//   - `RouteProbe`, M0-WP11's check of what a binding call to an object carries (C2).
//   - `Register` and `StaleRegister`, self-test targets: one is linearizable, the other serves stale reads on purpose,
//     so a run proves the checker accepts the first and catches the second.
//
// A target is any binding whose stub has `op(input: Op): Promise<Out>`. M0-WP2 adds its prototype edge here as one more
// binding; that redeploys only this script.

import { DurableObject } from "cloudflare:workers";
import { FaultControl, Faults, faultyStub, faultyWrite, type FaultPlan } from "./faults.ts";
export { FaultControl };

export type Op =
  | { kind: "put"; key: string; value: string }
  | { kind: "get"; key: string }
  | { kind: "cas"; key: string; expect: string | null; value: string };
export type Out = { value?: string | null; ok?: boolean };

/** One history row, in Porcupine's terms: `call` and `ret` in ms since the epoch, `ok: false` when no reply came. */
export interface HistoryRow { seq: number; client: number; colo: string; call: number; ret: number; op: Op; ok: boolean; out?: Out; error?: string }

export interface LoadSpec {
  run: string;
  /** Binding name of the target: a DO namespace (called by `object` name) or, with `service`, a service binding whose
   *  `op(op, object)` takes the name as its group (the M0-WP2 Raft edge). */
  target: string;
  service?: boolean;
  object?: string;
  /** Concurrent logical clients in this object; each issues its next op only after the last returns. */
  concurrency: number;
  /** Ops per second per logical client. */
  rate: number;
  durationMs: number;
  keys: number;
  /** Share of reads; the rest split evenly between put and cas. */
  readShare: number;
  /** Logical client ids start here, so ids stay unique across objects. */
  clientBase: number;
  callTimeoutMs?: number;
}

interface Env {
  M0_TOKEN: string;
  FAULTS: DurableObjectNamespace<FaultControl>;
  CLIENT: DurableObjectNamespace<LoadClient>;
  REGISTER: DurableObjectNamespace<Register>;
  STALE_REGISTER: DurableObjectNamespace<StaleRegister>;
  RAFT?: Fetcher;
  QUORUM: DurableObjectNamespace<QuorumProbe>;
  ROUTE: DurableObjectNamespace<RouteProbe>;
  [binding: string]: unknown;
}

const CHUNK_MS = 60_000; // one alarm runs at most this long, well inside the 15-minute alarm limit
const HISTORY_PAGE = 5_000;

async function colo(): Promise<string> {
  const trace = await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text();
  return /^colo=(\w+)$/m.exec(trace)?.[1] ?? "unknown";
}

let isolateFaults: Faults | undefined;
const faultsFor = (env: Env) => (isolateFaults ??= new Faults(env.FAULTS));

export class LoadClient extends DurableObject<Env> {
  private sql = this.ctx.storage.sql;
  private lastRead = new Map<string, string | null>();
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL)`);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS history (seq INTEGER PRIMARY KEY AUTOINCREMENT, run TEXT NOT NULL,
                   client INTEGER NOT NULL, call INTEGER NOT NULL, ret INTEGER NOT NULL, op TEXT NOT NULL,
                   ok INTEGER NOT NULL, out TEXT, error TEXT)`);
  }
  private meta(k: string) { return this.sql.exec<{ v: string }>(`SELECT v FROM meta WHERE k = ?`, k).toArray()[0]?.v; }
  private setMeta(k: string, v: string) { this.sql.exec(`INSERT OR REPLACE INTO meta VALUES (?, ?)`, k, v); }

  /** Creates the client and reports the colo it runs in, which never changes afterwards. */
  async place(): Promise<string> {
    let c = this.meta("colo");
    if (!c) { c = await colo(); this.setMeta("colo", c); }
    return c;
  }

  async start(spec: LoadSpec): Promise<void> {
    await this.place();
    this.setMeta("spec", JSON.stringify(spec));
    this.setMeta("end", String(Date.now() + spec.durationMs));
    await this.ctx.storage.setAlarm(Date.now());
  }

  async stop(): Promise<void> { this.setMeta("end", "0"); await this.ctx.storage.deleteAlarm(); }

  async alarm(): Promise<void> {
    const spec = JSON.parse(this.meta("spec")!) as LoadSpec;
    const end = Number(this.meta("end"));
    const chunkEnd = Math.min(end, Date.now() + CHUNK_MS);
    const ns = this.env[spec.target] as DurableObjectNamespace & Fetcher;
    // A stub that has thrown stays broken after its object resets, so each failed call gets a new one. The member-call
    // faults apply between client and target only in self-tests; in M0-WP2 the members inject them.
    // A DO namespace is called by object name. A service binding (the M0-WP2 Raft edge) gets the name as its group.
    const connect = () => faultyStub((spec.object !== undefined && !spec.service ? ns.getByName(spec.object) : ns) as unknown as
      { op(op: Op, group?: string): Promise<Out> }, faultsFor(this.env), spec.target);
    let target = connect();
    const gap = 1000 / spec.rate;
    // Values must be unique across the whole run, so a read names exactly one write. Each alarm chunk restarts `n`,
    // so the chunk's start time is part of every value.
    const chunk = Date.now().toString(36);
    let n = 0;
    const loop = async (client: number) => {
      while (Date.now() < chunkEnd) {
        const t0 = Date.now();
        const op = this.pick(spec, client, `${chunk}-${n++}`);
        const call = Date.now();
        let row: Omit<HistoryRow, "seq" | "colo">;
        try {
          const out = await Promise.race([target.op(op, spec.object), new Promise<never>((_, rej) =>
            setTimeout(() => rej(new Error("client timeout")), spec.callTimeoutMs ?? 10_000))]);
          row = { client, call, ret: Date.now(), op, ok: true, out };
          if (op.kind === "get") this.lastRead.set(`${client}/${op.key}`, out.value ?? null);
        } catch (e) {
          row = { client, call, ret: Date.now(), op, ok: false, error: String(e).slice(0, 200) };
          target = connect();
        }
        this.sql.exec(`INSERT INTO history (run, client, call, ret, op, ok, out, error) VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
          spec.run, row.client, row.call, row.ret, JSON.stringify(row.op), row.ok ? 1 : 0,
          row.out === undefined ? null : JSON.stringify(row.out), row.error ?? null);
        const wait = gap - (Date.now() - t0);
        if (wait > 0) await new Promise((r) => setTimeout(r, wait));
      }
    };
    await Promise.all(Array.from({ length: spec.concurrency }, (_, k) => loop(spec.clientBase + k)));
    if (Date.now() < end) await this.ctx.storage.setAlarm(Date.now());
  }

  private pick(spec: LoadSpec, client: number, i: string): Op {
    const key = `k${Math.floor(Math.random() * spec.keys)}`;
    const value = `${client}.${i}`; // unique, so a read names the write it saw
    const r = Math.random();
    if (r < spec.readShare) return { kind: "get", key };
    if (r < spec.readShare + (1 - spec.readShare) / 2) return { kind: "put", key, value };
    // cas expects the value this client last read for the key, or null for a key it never read
    return { kind: "cas", key, expect: this.lastRead.get(`${client}/${key}`) ?? null, value };
  }

  async status(): Promise<{ colo: string; rows: number; failed: number; end: number; running: boolean }> {
    const r = this.sql.exec<{ n: number; f: number }>(`SELECT count(*) AS n, coalesce(sum(1 - ok), 0) AS f FROM history`).one();
    const end = Number(this.meta("end") ?? 0);
    return { colo: this.meta("colo") ?? "", rows: r.n, failed: r.f, end, running: (await this.ctx.storage.getAlarm()) !== null };
  }

  async history(run: string, after: number): Promise<HistoryRow[]> {
    const c = this.meta("colo") ?? "";
    return this.sql.exec<{ seq: number; client: number; call: number; ret: number; op: string; ok: number; out: string | null; error: string | null }>(
      `SELECT seq, client, call, ret, op, ok, out, error FROM history WHERE run = ? AND seq > ? ORDER BY seq LIMIT ?`,
      run, after, HISTORY_PAGE).toArray().map((r) => ({ seq: r.seq, client: r.client, colo: c, call: r.call, ret: r.ret,
        op: JSON.parse(r.op), ok: r.ok === 1, ...(r.out ? { out: JSON.parse(r.out) } : {}), ...(r.error ? { error: r.error } : {}) }));
  }
}

/** A linearizable register per key in one object, with every write through the fault injector. */
export class Register extends DurableObject<Env> {
  protected sql = this.ctx.storage.sql;
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL, prev TEXT)`);
  }
  protected read(key: string): { v: string; prev: string | null } | undefined {
    return this.sql.exec<{ v: string; prev: string | null }>(`SELECT v, prev FROM kv WHERE k = ?`, key).toArray()[0];
  }
  async op(op: Op): Promise<Out> {
    if (op.kind === "get") return { value: this.read(op.key)?.v ?? null };
    return faultyWrite(this.ctx, faultsFor(this.env), `register:${op.kind}`, (): Out => {
      const cur = this.read(op.key);
      if (op.kind === "cas" && (cur?.v ?? null) !== op.expect) return { ok: false };
      this.sql.exec(`INSERT OR REPLACE INTO kv VALUES (?, ?, ?)`, op.key, op.value, cur?.v ?? null);
      return { ok: true };
    });
  }
  async reset(): Promise<void> { this.sql.exec(`DELETE FROM kv`); }
}

/** Wrong on purpose: a fifth of reads return the value before the latest write. */
export class StaleRegister extends Register {
  async op(op: Op): Promise<Out> {
    const cur = this.read(op.key);
    if (op.kind === "get" && cur && Math.random() < 0.2) return { value: cur.prev };
    return super.op(op);
  }
}

/** M0-WP11 (b): how far away a write's confirming followers are. A write is confirmed once 3 of its 5 storage followers
 *  hold it (https://blog.cloudflare.com/sqlite-in-durable-objects), so its confirmation time bounds the round trip to the
 *  third-nearest follower. Two readings: `sync()` after a write, and the output gate, which holds an outgoing fetch
 *  until earlier writes are confirmed, against the same fetch with no write before it. */
export class QuorumProbe extends DurableObject<Env> {
  private sql = this.ctx.storage.sql;
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS w (i INTEGER PRIMARY KEY, at INTEGER NOT NULL)`);
  }
  async measure(samples: number): Promise<{ colo: string; syncMs: number[]; gatedFetchMs: number[]; plainFetchMs: number[] }> {
    const here = await colo();
    const syncMs: number[] = [], gatedFetchMs: number[] = [], plainFetchMs: number[] = [];
    const trace = () => fetch("https://www.cloudflare.com/cdn-cgi/trace").then((r) => r.arrayBuffer());
    for (let i = 0; i < samples; i++) {
      this.sql.exec(`INSERT INTO w (at) VALUES (?)`, Date.now());
      let t0 = Date.now();
      await this.ctx.storage.sync();
      syncMs.push(Date.now() - t0);
      t0 = Date.now();
      await trace();
      plainFetchMs.push(Date.now() - t0);
      this.sql.exec(`INSERT INTO w (at) VALUES (?)`, Date.now());
      t0 = Date.now();
      await trace(); // held by the output gate until the insert above is confirmed
      gatedFetchMs.push(Date.now() - t0);
    }
    return { colo: here, syncMs, gatedFetchMs, plainFetchMs };
  }
}

/** M0-WP11 (a): what a Durable Object receives through a binding. A request that crossed the front-line proxy carries
 *  the headers it adds (`cf-ray`, `cf-connecting-ip`, `cf-visitor`, `x-forwarded-proto`); the caller compares them with
 *  the headers its own public request arrived with (decisions/OPEN-QUESTIONS.md, C2). */
export class RouteProbe extends DurableObject<Env> {
  async fetch(req: Request): Promise<Response> {
    return Response.json({ headers: Object.fromEntries(req.headers), cf: req.cf ?? null, colo: await colo() });
  }
}

const json = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status, headers: { "content-type": "application/json" } });

export default {
  async fetch(req, env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return json({ error: "unauthorized" }, 401);
    const path = new URL(req.url).pathname;
    const body = req.method === "POST" ? await req.json<Record<string, any>>() : {};
    const names = (body.names ?? []) as string[];
    const client = (n: string) => env.CLIENT.getByName(n);
    const all = <T>(xs: Promise<T>[]) => Promise.allSettled(xs).then((rs) =>
      rs.map((r) => (r.status === "fulfilled" ? r.value : { error: String(r.reason) })));
    switch (path) {
      case "/faults": {
        const fc = env.FAULTS.getByName("plan");
        if (req.method === "POST") await fc.set(body.plan as FaultPlan);
        return json(await fc.get());
      }
      // A location hint is respected only by the first call, so `place` creates each client with its hint.
      case "/clients/place": return json(await all(names.map((n) =>
        env.CLIENT.get(env.CLIENT.idFromName(n), { locationHint: body.hint }).place())));
      case "/clients/start": return json(await all(names.map((n, i) =>
        client(n).start({ ...(body.spec as LoadSpec), clientBase: i * 100 }))));
      case "/clients/stop": return json(await all(names.map((n) => client(n).stop())));
      case "/clients/status": return json(await all(names.map((n) => client(n).status())));
      case "/clients/history": return json(await client(body.name).history(body.run, body.after ?? 0));
      case "/quorum": return json(await all(names.map((n) =>
        env.QUORUM.get(env.QUORUM.idFromName(n), { locationHint: body.hint }).measure(body.samples ?? 20))));
      case "/route": {
        const stub = env.ROUTE.get(env.ROUTE.idFromName(body.name ?? "route"), { locationHint: body.hint });
        const t0 = Date.now();
        const seen = await (await stub.fetch("https://route.internal/", { headers: { "x-m0": "1" } })).json();
        const ms = [] as number[];
        for (let i = 0; i < 10; i++) { const t = Date.now(); await (await stub.fetch("https://route.internal/")).arrayBuffer(); ms.push(Date.now() - t); }
        return json({ workerColo: (req.cf as { colo?: string } | undefined)?.colo, workerSaw: Object.fromEntries(req.headers),
          objectSaw: seen, firstCallMs: Date.now() - t0, bindingRoundTripsMs: ms });
      }
      // B0-17 in one account: a public HTTPS hop to another Worker of this account, which reaches it only through the
      // front door when the caller has `global_fetch_strictly_public`.
      case "/hop": {
        const ms: number[] = []; let status = 0, text = "";
        for (let i = 0; i < 10; i++) {
          const t = Date.now();
          const r = await fetch(body.url, { headers: { authorization: `Bearer ${env.M0_TOKEN}` } });
          status = r.status; text = (await r.text()).slice(0, 200); ms.push(Date.now() - t);
        }
        return json({ workerColo: (req.cf as { colo?: string } | undefined)?.colo, status, text, ms });
      }
      case "/register/reset": {
        await env.REGISTER.getByName(body.object).reset();
        await env.STALE_REGISTER.getByName(body.object).reset();
        return json({ ok: true });
      }
    }
    return json({ error: "not found" }, 404);
  },
} satisfies ExportedHandler<Env>;
