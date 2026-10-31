// graft-m0-chaos: B0-11's chaos loop (run/raft-chaos.ts, `chaos()`) as a Durable Object alarm, so the resets and
// transfers keep going while the orb that started the run is asleep. It calls only the Raft edge's public routes
// (`/status`, `/reset`, `/transfer`, `/faults`) with the M0 bearer token, so it never deploys or binds a `graft-raft-*`
// script. Each alarm takes one step, the same one the local loop takes, and logs it; `run/raft-chaos.ts --resume` reads
// the log back into the run's events file and skips its own loop while this one runs.
//
//   POST /start  {run, url, group, end, every}   POST /stop {run}   POST /events {run, after}

import { DurableObject } from "cloudflare:workers";

interface Env { CHAOS: DurableObjectNamespace<ChaosDriver>; M0_TOKEN: string }
type Cfg = { run: string; url: string; group: string; end: number; every: number };
type Ev = { at: number; what: string; result: string };

export class ChaosDriver extends DurableObject<Env> {
  private sql = this.ctx.storage.sql;
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql.exec("CREATE TABLE IF NOT EXISTS ev (seq INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER, what TEXT, result TEXT)");
  }

  private log(e: Ev): void { this.sql.exec("INSERT INTO ev (at, what, result) VALUES (?, ?, ?)", e.at, e.what, e.result); }

  private async post<T>(cfg: Cfg, path: string, body: object): Promise<T> {
    const res = await fetch(cfg.url + path, { method: "POST", body: JSON.stringify({ group: cfg.group, ...body }),
      headers: { authorization: `Bearer ${this.env.M0_TOKEN}` }, signal: AbortSignal.timeout(40_000) });
    const text = await res.text();
    if (!res.ok) throw new Error(`${res.status} ${text.slice(0, 200)}`);
    return JSON.parse(text) as T;
  }

  async start(cfg: Cfg): Promise<object> {
    await this.ctx.storage.put("cfg", cfg);
    this.log({ at: Date.now(), what: "remote chaos driver started", result: new Date(cfg.end).toISOString() });
    await this.ctx.storage.setAlarm(Date.now() + 1000);
    return { started: true, end: new Date(cfg.end).toISOString() };
  }

  async stop(): Promise<object> {
    await this.ctx.storage.deleteAlarm();
    await this.ctx.storage.delete("cfg");
    this.log({ at: Date.now(), what: "remote chaos driver stopped", result: "ok" });
    return { stopped: true };
  }

  async events(after: number): Promise<object> {
    const rows = this.sql.exec("SELECT seq, at, what, result FROM ev WHERE seq > ? ORDER BY seq LIMIT 2000", after).toArray();
    return { running: (await this.ctx.storage.getAlarm()) !== null, rows };
  }

  /** One step of run/raft-chaos.ts `chaos()`: 60% a reset (half of them the leader), else a transfer to a random site,
   *  30% of transfers abandoned. Stops 10 s before the end and clears the fault plan, as the local loop does. */
  async alarm(): Promise<void> {
    const cfg = await this.ctx.storage.get<Cfg>("cfg");
    if (!cfg) return;
    if (Date.now() >= cfg.end - 10_000) {
      await this.post(cfg, "/faults", { plan: { rules: [] } }).catch(() => undefined);
      this.log({ at: Date.now(), what: "remote chaos driver ended; faults cleared", result: "ok" });
      await this.ctx.storage.delete("cfg");
      return;
    }
    const sites = ["a", "b", "c"];
    try {
      if (Math.random() < 0.6) {
        const st = await this.post<{ site: string; role: string }[]>(cfg, "/status", {}).catch(() => []);
        const leader = st.find((x) => x.role === "leader")?.site;
        const s = leader && Math.random() < 0.5 ? leader : sites[Math.floor(Math.random() * 3)];
        await this.post(cfg, "/reset", { sites: [s] });
        this.log({ at: Date.now(), what: `reset ${s}`, result: "ok" });
      } else {
        const to = sites[Math.floor(Math.random() * 3)];
        const abandon = Math.random() < 0.3;
        const r = await this.post<{ from: string; result: string }>(cfg, "/transfer", { to, abandon });
        this.log({ at: Date.now(), what: `transfer ${r.from}->${to}${abandon ? " (abandon)" : ""}`, result: r.result });
      }
    } catch (e) {
      this.log({ at: Date.now(), what: "chaos step", result: String(e).slice(0, 120) });
    }
    await this.ctx.storage.setAlarm(Date.now() + cfg.every * 1000 * (1 + Math.random()));
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    const path = new URL(req.url).pathname;
    const body = await req.json<Cfg & { after?: number }>();
    const stub = env.CHAOS.get(env.CHAOS.idFromName(body.run));
    if (path === "/start") return Response.json(await stub.start(body));
    if (path === "/stop") return Response.json(await stub.stop());
    if (path === "/events") return Response.json(await stub.events(body.after ?? 0));
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<Env>;
