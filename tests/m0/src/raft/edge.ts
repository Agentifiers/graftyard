// M0-WP2 prototype edge, `graft-raft-edge`. It binds every site's member class with `script_name` and runs the
// caller's side of the write and read paths (design/DESIGN.md, "Commit and read paths"): it mints one request id per
// client operation and reuses it on every re-send, follows `NotLeader`, gives every receipt CALL_DEADLINE, and hedges
// only with a `suspect` report to a follower on another site, never by forwarding the command.
//
// It is a `RaftEdge` entrypoint so graft-m0-tools' load clients reach it through a service binding with `op()`, and
// it exposes the same over HTTP for runners.

import { WorkerEntrypoint } from "cloudflare:workers";
import { Faults, faultyStub, type FaultControl } from "../faults.ts";
import { call, type CommitRes, type OutcomeRes, type RaftMember, type ReadRes } from "./member.ts";
import { CALL_DEADLINE, COMMIT_RETRIES, hintFor, SITES, learnerName, memberName, type MemberStatus, type Out, type RaftOp, type SiteKey } from "./types.ts";
import { healthObject, recordCall, type SiteHealth } from "./health.ts";

interface EdgeEnv {
  M0_TOKEN: string;
  MEMBER_A: DurableObjectNamespace<RaftMember>;
  MEMBER_B: DurableObjectNamespace<RaftMember>;
  MEMBER_C: DurableObjectNamespace<RaftMember>;
  FAULTS?: DurableObjectNamespace<FaultControl>;
  PLAN?: string;
  /** M0-WP4: each site's `SiteHealth` class, read by `/health`. */
  HEALTH_A?: DurableObjectNamespace<SiteHealth>;
  HEALTH_B?: DurableObjectNamespace<SiteHealth>;
  HEALTH_C?: DurableObjectNamespace<SiteHealth>;
}

/** Thrown when the edge gives up on a client operation; the operation's effect is unknown. */
export class EdgeGaveUp extends Error {}

const OP_DEADLINE = 30_000;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const leaderCache = new Map<string, SiteKey>(); // per isolate, from the last reply that named a leader
let isolateFaults: Faults | undefined;

export class RaftEdge extends WorkerEntrypoint<EdgeEnv> {
  private faults = (isolateFaults ??= new Faults(this.env.FAULTS, Math.random, this.env.PLAN ?? "plan"));
  private colo = "edge";
  /** M0-WP4: the site this invocation stands in for as a `SiteHealth` observer (an edge on that site's continent). */
  private observer: SiteKey | null = null;
  private seen(target: SiteKey, ok: boolean): void {
    if (this.observer) recordCall(this.env[`HEALTH_${this.observer.toUpperCase()}` as "HEALTH_A"], this.observer, target, ok);
  }
  private notes: string[] = [];
  /** Per-operation trace, by request id, so a slow operation's own steps can be returned (M0-WP4). */
  private traces = new Map<string, string[]>();
  private note(m: string, rid?: string): void {
    if (this.notes.length < 400) this.notes.push(`${Date.now() % 100000} ${m}`);
    if (rid) this.traces.get(rid)?.push(`${Date.now() % 100000} ${m}`);
  }

  private member(group: string, site: SiteKey): DurableObjectStub<RaftMember> {
    const ns = this.env[`MEMBER_${site.toUpperCase()}` as "MEMBER_A"];
    return faultyStub(ns.get(ns.idFromName(memberName(group, site)), { locationHint: hintFor(group, site) }), this.faults, `edge>${site}`);
  }

  /** Hedge after a missed receipt: a `suspect` report to each follower on another site, never the command itself. */
  /** `answered`: the target answered NotLeader naming no leader, so it is a follower that knows of none, not a gray
   *  leader, and it is reported to as well. It may hold the most current log after a reset deposed it. */
  private async hedge(group: string, from: SiteKey, rid?: string, answered = false): Promise<void> {
    // Returns as soon as one follower leads; a hung follower must not hold the client's next attempt.
    const to = SITES.filter((x) => answered || x !== from);
    await new Promise<void>((resolve) => {
      let pending = to.length;
      for (const s of to) {
        call(() => this.member(group, s).suspect(group, this.colo, from), CALL_DEADLINE, 0).then((v) => {
          if (rid) this.note(`suspect ${from} to ${s}: ${v}`, rid);
          if (v === "leader") { leaderCache.set(group, s); resolve(); }
        }, (e) => { if (rid) this.note(`suspect ${from} to ${s} threw: ${String(e).slice(0, 60)}`, rid); }).finally(() => { if (--pending === 0) resolve(); });
      }
    });
  }

  /** Runs one client operation to completion through whichever member leads. Writes go through `commit` and
   *  `outcome`; reads through `read` (`fresh`).
   *
   *  `strict` holds the operation to the design's per-call budget (B0-25): each commit-path member call (`commit`,
   *  `outcome`, `read`) that throws, misses its deadline, or answers `NotLeader` without naming another leader is a
   *  failed attempt of that call, and the edge gives up when one call has failed 1 + COMMIT_RETRIES times. Following a
   *  named leader is free twice. Otherwise the edge keeps trying for OP_DEADLINE,
   *  as a client that re-sends would (B0-01, B0-03, B0-11). */
  async op(op: RaftOp, group = "g0", rid = crypto.randomUUID(), strict = false): Promise<Out> {
    const end = Date.now() + OP_DEADLINE;
    let target = leaderCache.get(group) ?? "a";
    this.traces.set(rid, [`start ${group} at ${target}`]);
    
    let tried = 0, redirects = 0, method = "commit";
    const failures: Record<string, number> = {};
    const missed = (named: SiteKey | null) => {
      if (!strict) return;
      if (named && named !== target && ++redirects <= 2) return;
      failures[method] = (failures[method] ?? 0) + 1;
      if (failures[method] > COMMIT_RETRIES) throw new EdgeGaveUp(`gave up on ${op.kind} ${op.key}: ${method} failed ${failures[method]} times`);
    };
    while (Date.now() < end) {
      const stub = this.member(group, target);
      try {
        if (op.kind === "get") {
          const r = await call<ReadRes>(() => stub.read(group, op.key), CALL_DEADLINE * 2, 0);
          this.seen(target, true);
          if ("out" in r) { leaderCache.set(group, target); return r.out; }
          if ("busy" in r) { missed(null); await sleep(50); continue; }
          this.note(`read notLeader from ${target}: ${JSON.stringify(r.notLeader)}`, rid);
          missed(r.notLeader.leader);
          if (!r.notLeader.leader) await this.hedge(group, target, rid, true);
          target = this.follow(group, r.notLeader.leader, target, ++tried);
          if (tried % 3 === 0) await sleep(100);
          continue;
        }
        method = "commit";
        const tc = Date.now();
        const c = await call<CommitRes>(() => stub.commit(group, rid, op), CALL_DEADLINE, 0);
        this.seen(target, true);
        if (Date.now() - tc > 500) this.note(`commit at ${target} took ${Date.now() - tc} ms`, rid);
        if ("done" in c) { leaderCache.set(group, target); return c.done; }
        if ("notLeader" in c) {
          this.note(`commit notLeader from ${target}: ${JSON.stringify(c.notLeader)}`, rid);
          missed(c.notLeader.leader);
          // A member that names no leader knows of none: report it, as after a missed deadline, so a follower whose
          // promise has lapsed campaigns. Otherwise a deposed leader and a lost campaign leave no one to campaign.
          if (!c.notLeader.leader) await this.hedge(group, target, rid, true);
          target = this.follow(group, c.notLeader.leader, target, ++tried);
          if (tried % 3 === 0) await sleep(100);
          continue;
        }
        leaderCache.set(group, target);
        method = "outcome";
        const to = Date.now();
        const o = await call<OutcomeRes>(() => stub.outcome(group, c.admitted), 20_000, 0);
        if (Date.now() - to > 1_000) this.note(`outcome at ${target} took ${Date.now() - to} ms`, rid);
        if ("out" in o) return o.out;
        this.note(`outcome notLeader from ${target}: ${JSON.stringify(o.notLeader)}`, rid);
        // A deposed term: re-send with the same request id; the new leader decides it, or answers it from `req`.
        missed(o.notLeader.leader);
        target = this.follow(group, o.notLeader.leader, target, ++tried);
      } catch (e) {
        if (e instanceof EdgeGaveUp) throw e;
        this.seen(target, false);
        this.note(`${op.kind === "get" ? "read" : method} to ${target} threw: ${String(e).slice(0, 80)}`, rid);
        missed(null);
        await this.hedge(group, target, rid);
        target = this.follow(group, null, target, ++tried);
        await sleep(100);
      }
    }
    throw new EdgeGaveUp(`gave up on ${op.kind} ${op.key} after ${OP_DEADLINE} ms`);
  }

  private follow(group: string, named: SiteKey | null, current: SiteKey, tried: number): SiteKey {
    if (named && named !== current) { leaderCache.set(group, named); return named; }
    leaderCache.delete(group);
    void tried; // callers back off every third miss
    return SITES[(SITES.indexOf(current) + 1) % SITES.length];
  }

  async status(group = "g0"): Promise<(MemberStatus | { error: string })[]> {
    return Promise.all(SITES.map((s) => call(async () => (await this.member(group, s).status()) as unknown as MemberStatus, 3_000, 0)
      .catch((e) => ({ error: String(e) }))));
  }

  async fetch(req: Request): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${this.env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    this.colo = (req.cf as { colo?: string } | undefined)?.colo ?? "edge";
    const url = new URL(req.url);
    const body = req.method === "POST" ? await req.json<Record<string, any>>() : {};
    const group = (body.group ?? url.searchParams.get("group") ?? "g0") as string;
    const json = (v: unknown, status = 200) => Response.json(v, { status });
    try {
      switch (url.pathname) {
        case "/op": {
          // `learner: slot` sends a get to that learner instead (B0-11 with learners, B0-18).
          if (body.learner !== undefined && (body.op as RaftOp).kind === "get") {
            const slot = Number(body.learner), site = SITES[slot % 3];
            const ns = this.env[`MEMBER_${site.toUpperCase()}` as "MEMBER_A"];
            const r = await call<ReadRes>(() => ns.get(ns.idFromName(learnerName(group, slot)), { locationHint: hintFor(group, site) })
              .learnerRead(group, (body.op as RaftOp).key), CALL_DEADLINE * 5, 0);
            if ("out" in r) return json({ out: r.out });
            return json({ error: JSON.stringify(r) }, 503);
          }
          return json({ out: await this.op(body.op as RaftOp, group, undefined, !!body.strict) });
        }
        case "/status": return json(await this.status(group));
        // Many groups' status in one call, for the site-day runner.
        case "/statusMany": {
          // `sites` limits it to those sites' members.
          const sites = (body.sites ?? SITES) as SiteKey[];
          return json(Object.fromEntries(await Promise.all((body.groups as string[]).map(async (g) => [g, await Promise.all(sites.map((s) =>
            call(async () => (await this.member(g, s).status()) as unknown as MemberStatus, 5_000, 0).catch((e) => ({ site: s, error: String(e) }))))]))));
        }
        case "/health": {
          // M0-WP4: every (observer, target) SiteHealth count over `windowMs`.
          const out: Record<string, { calls: number; missed: number } | { error: string }> = {};
          await Promise.all(SITES.flatMap((o) => SITES.filter((t) => t !== o).map(async (t) => {
            const ns = this.env[`HEALTH_${o.toUpperCase()}` as "HEALTH_A"]!;
            out[`${o}>${t}`] = await call(() => healthObject(ns, o, t).read(body.windowMs ?? 30_000, !!body.series), 5_000, 0).catch((e) => ({ error: String(e) }));
          })));
          return json(out);
        }
        case "/faults": {
          // The same plan graft-m0-tools serves; set here too so a run needs one URL.
          const fc = this.env.FAULTS!.getByName(this.env.PLAN ?? "plan");
          await fc.set(body.plan);
          return json({ at: Date.now(), plan: await fc.get() });
        }
        case "/transfer": {
          const st = await this.status(group);
          const leader = st.find((s): s is MemberStatus => "role" in s && s.role === "leader");
          if (!leader) return json({ error: "no leader" }, 409);
          return json({ from: leader.site, result: await this.member(group, leader.site).transfer(group, body.to, !!body.abandon) });
        }
        // Every test-control route gives each member call a deadline, so a site still blackholed by a cached plan
        // costs the caller seconds, not the run.
        case "/stats": return json(await Promise.all(SITES.map((s) => call(() => this.member(group, s).stats(!!body.reset), 10_000, 0)
          .catch((e) => ({ site: s, error: String(e) })))));
        case "/connprobe": return json(await this.member(group, body.site).connProbe(group, body.slow, body.fast, body.n ?? 6, body.ms ?? 5_000));
        case "/syncprobe": return json(await Promise.all(SITES.map((s) => this.member(group, s).syncProbe(body.ms ?? 30_000, body.every ?? 50, body.bytes, body.alarmEvery)
          .catch((e) => ({ error: String(e) })))));
        case "/probe": return json(await Promise.all(SITES.map((s) => this.member(group, s).probe(group, body.n ?? 20).catch((e) => ({ error: String(e) })))));
        case "/fanout": {
          // How many member calls one invocation runs at once: n concurrent pings to one site, each timed.
          const site = (body.site ?? "b") as SiteKey, n = body.n ?? 20, t0 = Date.now();
          const ms = await Promise.all(Array.from({ length: n }, async () => {
            const t = Date.now(); await this.member(group, site).ping(); return [t - t0, Date.now() - t0];
          }));
          return json({ colo: this.colo, ms });
        }
        case "/learner": {
          // B0-18: one learner (slot `slot`, on site slot mod 3, created with that site's hint) runs `rate` reads/s.
          const slot = Number(body.slot ?? 0), site = SITES[slot % 3];
          const ns = this.env[`MEMBER_${site.toUpperCase()}` as "MEMBER_A"];
          const stub = ns.get(ns.idFromName(learnerName(group, slot)), { locationHint: hintFor(group, site) });
          const r = await stub.selfLoad(group, body.seconds ?? 20, body.rate ?? 1000, body.keys ?? 64);
          const colo = (await stub.status()).colo;
          return json({ slot, site, colo, ...r });
        }
        // Lease checks: one member's own read or write, with no routing, so a test can ask a member that should not
        // lead whether it still answers.
        case "/readAt": return json(await call<ReadRes>(() => this.member(group, body.site).read(group, body.key), 5_000, 0));
        case "/commitAt": {
          const m = this.member(group, body.site), c = await call<CommitRes>(() => m.commit(group, crypto.randomUUID(), body.op), 5_000, 0);
          return json("admitted" in c ? await call<OutcomeRes>(() => m.outcome(group, c.admitted), 10_000, 0) : c);
        }
        // Ticket checks: a member's raw `commit` reply and `outcome` for any ticket.
        case "/commitRaw": return json(await call<CommitRes>(() => this.member(group, body.site).commit(group, body.rid ?? crypto.randomUUID(), body.op), 5_000, 0));
        case "/outcomeRaw": return json(await call<OutcomeRes>(() => this.member(group, body.site).outcome(group, body.ticket), 20_000, 0));
        case "/dump": return json(await Promise.all(SITES.map((s) => call(() => this.member(group, s).dump(group, body.from, body.to), 20_000, 0)
          .then((d) => ({ site: s, ...d }), (e) => ({ site: s, error: String(e) })))));
        case "/reset": {
          await Promise.allSettled((body.sites as SiteKey[]).map((s) => this.member(group, s).reset()));
          return json({ ok: true });
        }
        case "/bench": {
          // `colo` labels this invocation's `suspect` reports, so one runner can stand in for edges in two colos (B0-26).
          if (body.colo) this.colo = body.colo;
          if (body.observer) this.observer = body.observer;
          // M0-WP4: `groups` spreads the load over many groups, a random one per operation.
          const groups: string[] = body.groups ?? [group];
          // B0-01 and B0-02 from inside the edge: `n` puts of `bytes` each, `concurrency` at a time.
          const n = body.n ?? 100, conc = body.concurrency ?? 10, value = "x".repeat(body.bytes ?? 1024);
          const lat: number[] = []; let i = 0, failed = 0; const errors: Record<string, number> = {};
          const slow: { group: string; ms: number; trace: string[] }[] = [];
          const ops: [number, number, string, boolean, string?][] = []; // start, end, kind, ok (B0-03), and group when `groups` is set
          const readShare = body.readShare ?? 0;
          const t0 = Date.now();
          await Promise.all(Array.from({ length: conc }, async () => {
            // A random phase per worker, so paced workers do not start, and stay, in lockstep.
            if (body.ratePerWorker) await sleep(Math.random() * 1000 / body.ratePerWorker);
            while (i < n) {
              const k = i++;
              const t = Date.now();
              // `newShare` of puts write the rare "new:" kind, which a WP4 release writes in its new record shape.
              const key = `${Math.random() < (body.newShare ?? 0) ? "new:" : ""}bench${k % 64}`;
              const op: RaftOp = Math.random() < readShare ? { kind: "get", key: `bench${k % 64}` } : { kind: "put", key, value };
              const g = groups[Math.floor(Math.random() * groups.length)];
              const tag = body.groups ? [g] as [string] : [] as [];
              const rid = crypto.randomUUID();
              const traced = () => { const tr = this.traces.get(rid); this.traces.delete(rid); if (Date.now() - t > 5_000 && tr && slow.length < 20) slow.push({ group: g, ms: Date.now() - t, trace: tr }); };
              try { await this.op(op, g, rid); traced(); lat.push(Date.now() - t); if (body.ops) ops.push([t, Date.now(), op.kind, true, ...tag]); }
              catch (e) { traced(); failed++; const m = String(e).slice(0, 100); errors[m] = (errors[m] ?? 0) + 1; if (body.ops) ops.push([t, Date.now(), op.kind, false, ...tag]); }
              if (body.ratePerWorker) await sleep(Math.max(0, 1000 / body.ratePerWorker - (Date.now() - t)));
            }
          }));
          return json({ colo: this.colo, n, failed, wallMs: Date.now() - t0, lat, errors, notes: this.notes.slice(0, 400), slow, ...(body.ops ? { ops } : {}) });
        }
      }
    } catch (e) {
      return json({ error: String(e), notes: this.notes.slice(-12) }, 500);
    }
    return json({ error: "not found" }, 404);
  }
}

export default RaftEdge;
