// M0-WP9 load script, `graft-wp9-load` (build/BUILD-PLAN.md, M0-WP9, rows B0-29, B0-32, B0-33). It binds the three
// graft-wp9 member classes with `script_name` and holds `Load9`, a load client created with a location hint and run
// from its alarm, as src/rehydrate.ts's CI clients are. Each client plays edges for its share of journey 9 (design/
// DESIGN.md, journey 9; design/AGENT-LAYER.md, journey A1): agents pushing their own refs with inline packs, agent
// side commands, humans, a bot, the merge queue, ref reads, API reads, and activity-feed long-polls.
//
// The client runs the caller's side of the write path itself (design/DESIGN.md, "Commit and read paths"): one request id
// per operation, reused on every re-send, `commit` with CALL_DEADLINE for the receipt, `outcome` for the result,
// following `NotLeader`, and a `suspect` report to the other members after a missed receipt, never a forwarded
// command. It classes each command (`AdmitClass`), stages a pack in the `graft-wp9-stage` bucket before it sends when
// the leader's last reply asked (hot inline cap, or agent pressure), and routes feed polls to learners first and to
// followers it does not believe lead, never to the leader. A Durable Object client is not held to the six-calls-at-once
// limit of a Worker invocation: 200 concurrent RPCs from one object finished in about 1 s (measured 2026-10-29).

import { DurableObject } from "cloudflare:workers";
import type { CommitRes, FeedRes, OutcomeRes, RaftMember, ReadRes } from "./member.ts";
import {
  CALL_DEADLINE, COMMIT_HOLD, COMMIT_RETRIES, AGENT_WAIT, FEED_WAIT_MAX, INLINE_MAX_HOT, REFUSAL_SHAPE, SITES, hintFor, learnerName, memberName,
  type AdmitClass, type Hints, type Meta, type RaftOp, type SiteKey, variant,
} from "./types.ts";

interface Env {
  M0_TOKEN: string;
  MEMBER_A: DurableObjectNamespace<RaftMember>;
  MEMBER_B: DurableObjectNamespace<RaftMember>;
  MEMBER_C: DurableObjectNamespace<RaftMember>;
  LOAD: DurableObjectNamespace<Load9>;
  STAGE: R2Bucket;
  /** The staging bucket on apac, for runs whose leader is on site c. */
  STAGE_C: R2Bucket;
}

export interface Spec {
  run: string; group: string; index: number; clients: number; start: number; seconds: number;
  /** Fleet size. Each agent pushes its own ref every `pushEvery` s. Pushes and merges come only from the clients in
   *  `pushers` (created with the enam hint), because a push reaches the leader from the network's git cache shard on
   *  site A (design/DESIGN.md, journey 1, step 2), not from the edge that took it; agent i belongs to pushers[i % n]. */
  agents: number; pushEvery: number; pushers: number[];
  /** The site the group's leader is on (B0-31: "c", apac); staging goes to that site's bucket. Default "a". */
  leaderSite?: SiteKey;
  /** Inline pack sizes: `bigShare` of pushes are `bigBytes`, the rest uniform in [packMin, packMax]; `pushFixed` overrides. */
  packMin: number; packMax: number; bigShare: number; bigBytes: number; pushFixed?: number;
  /** Whole-fleet rates a second, split evenly over the clients. */
  agentSide: number; human: number; bot: number; refReads: number; apiReads: number;
  /** Agent REST, MCP, push, clone, and PathSet reads on the leader (design/AGENT-LAYER.md, journey A1 step 4: 43/s). */
  agentReads: number;
  /** Merge-queue merges a minute, chained by parent tip, from client 0 only. */
  mergesPerMin: number;
  /** Feed pollers in the fleet; even ones send `paths`. */
  pollers: number;
  /** Learner slots that serve feed polls; empty sends polls to followers. */
  learners: number[];
  /** One user token writing this many commands a minute, classed Human by the edge (client 1, or 0 if alone). */
  fastUser?: number;
  /** Third pass (B0-31): Human pushes a second over the fleet, each of a pack uniform in [`bigMin`, 1 MiB], from the
   *  pusher clients; merges then carry packs of the same sizes. */
  humanPushes?: number; bigMin?: number;
  /** A burst at `at` s: `pushes` agent pushes of `bytes` each over 1 s from the whole fleet, plus `merges` merges. */
  burst?: { at: number; pushes: number; bytes: number; merges: number };
}

/** One command row: [t ms from start, kind 0 push 1 merge 2 side, class 0 Human 1 MergeQueue 2 Bot 3 Agent, client ms,
 *  result 0 ok 1 stale 2 refused 3 error 4 other-false, leader commit ms (-1 if none), entries waited, staged 0/1, bytes,
 *  1 if from the fast user]. */
type Row = number[];
const KIND = { push: 0, merge: 1, side: 2 } as const;
const CLS: AdmitClass[] = ["Human", "MergeQueue", "Bot", "Agent"];
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const OP_DEADLINE = AGENT_WAIT + 30_000;

/** Calls `fn` with a deadline; rejects with "deadline" when it passes. */
function within<T>(fn: () => Promise<T>, ms: number): Promise<T> {
  let t: ReturnType<typeof setTimeout>;
  return Promise.race([fn(), new Promise<never>((_, rej) => { t = setTimeout(() => rej(new Error("deadline")), ms); })]).finally(() => clearTimeout(t));
}

/** The caller's side of the write path, the read path, and feed polls, for one group. One per client object. */
class Caller {
  leader: SiteKey = "a";
  hints: Hints = { hot: false, pressure: false, placeOk: true };
  bucket: R2Bucket | null = null;
  suspectsSent = 0;
  notLeaders = 0;
  gaveUp = 0;
  fallbacks = 0;
  errors: Record<string, number> = {};
  stageMs: number[] = [];
  /** Eighth pass: [epoch ms at stage start, stage ms, bytes] per stage, for B0-32's burst window. */
  stages: number[][] = [];
  constructor(private env: Env, private group: string) {}
  /** One stub per member, so calls through it keep their order (E-order); replaced after a call through it fails. */
  private stubs = new Map<SiteKey, DurableObjectStub<RaftMember>>();
  member(site: SiteKey): DurableObjectStub<RaftMember> {
    let s = this.stubs.get(site);
    if (!s) {
      const ns = this.env[`MEMBER_${site.toUpperCase()}` as "MEMBER_A"];
      s = ns.get(ns.idFromName(memberName(this.group, site)), { locationHint: hintFor(this.group, site) });
      this.stubs.set(site, s);
    }
    return s;
  }
  private broken(site: SiteKey): void { this.stubs.delete(site); }
  learner(slot: number): DurableObjectStub<RaftMember> {
    const site = SITES[slot % 3], ns = this.env[`MEMBER_${site.toUpperCase()}` as "MEMBER_A"];
    return ns.get(ns.idFromName(learnerName(this.group, slot)), { locationHint: hintFor(this.group, site) });
  }
  /** Errors by message and target, with the first time each was seen. */
  firstErr: Record<string, number> = {};
  private err(m: string): void { const k = m.slice(0, 90); this.errors[k] = (this.errors[k] ?? 0) + 1; this.firstErr[k] ??= Date.now(); }
  /** One hedge in flight per client: concurrent misses on the same leader share it. */
  private hedging: Promise<void> | null = null;
  /** `colo` labels this client's reports, as an edge's colo does. */
  colo = "wp9";
  /** A missed receipt: `suspect` to each other member, never the command (design/DESIGN.md, `suspect`). */
  private async hedge(from: SiteKey): Promise<void> {
    this.hedging ??= Promise.allSettled(SITES.filter((s) => s !== from).map(async (s) => {
      this.suspectsSent++;
      const v = await within(() => this.member(s).suspect(this.group, this.colo, from), CALL_DEADLINE);
      if (v === "leader") this.leader = s;
    })).then(() => undefined).finally(() => { this.hedging = null; });
    await this.hedging;
  }
  private follow(named: SiteKey | null): void { this.notLeaders++; this.leader = named && named !== this.leader ? named : SITES[(SITES.indexOf(this.leader) + 1) % 3]; }

  async stage(pack: Uint8Array): Promise<void> {
    const t = Date.now();
    // `PackStore::place` fails in a "nop-" group (B0-31): no bucket accepts the pack.
    if (variant(this.group, "nop") !== null) throw new Error("place failed: every bucket refused");
    await (this.bucket ?? this.env.STAGE).put(`${this.group}/${crypto.randomUUID()}`, pack);
    if (this.stageMs.length < 20_000) this.stageMs.push(Date.now() - t);
    if (this.stages.length < 20_000) this.stages.push([t, Date.now() - t, pack.byteLength]);
  }

  /** One write. Returns the result code, the leader's admission-to-ack time, the entries waited, and whether it staged. */
  /** Operations stop retrying at this time, so a client never keeps calling after its run. */
  stopAt = Infinity;
  async write(op: RaftOp, meta: Meta): Promise<{ code: number; commitMs: number; waited: number; staged: boolean; cls: AdmitClass; text?: string }> {
    const rid = crypto.randomUUID(), end = Math.min(this.stopAt, Date.now() + OP_DEADLINE);
    let staged = false;
    const pack = op.kind === "cas" ? op.pack : undefined;
    // Stage first when the leader's last reply said so: a hot group inlines only up to INLINE_MAX_HOT, and under agent
    // pressure only stored agent pushes queue.
    const unstaged = (): RaftOp => { const { pack: _p, ...rest } = op as RaftOp & { pack?: Uint8Array }; void _p; return rest as RaftOp; };
    const ih = variant(this.group, "ih"), inlineMaxHot = ih !== null ? ih << 10 : INLINE_MAX_HOT;
    if (pack && this.hints.placeOk && ((this.hints.hot && pack.byteLength > inlineMaxHot) || (meta.cls === "Agent" && this.hints.pressure))) {
      await this.stage(pack); staged = true; op = unstaged(); meta = { ...meta, stored: pack.byteLength };
    }
    // The design's per-call budget (as B0-25's `strict` edge): each commit-path call (`commit`, `outcome`) that throws,
    // misses its deadline, or answers NotLeader naming no other leader is a failed attempt of that call; the edge gives
    // up when one call has failed 1 + COMMIT_RETRIES times, and follows a named leader for free at most twice. The client
    // then sees an error (git: a 503) and the agent tries again at its next push.
    const failures: Record<string, number> = {};
    let redirects = 0;
    const missed = (method: string, named: SiteKey | null, site: SiteKey) => {
      if (named && named !== site && ++redirects <= 2) return false;
      return (failures[method] = (failures[method] ?? 0) + 1) > COMMIT_RETRIES;
    };
    const gaveUp = () => { this.gaveUp++; return { code: 3, commitMs: -1, waited: 0, staged, cls: meta.cls }; };
    const done = (o: Exclude<OutcomeRes, { notLeader: unknown }>) => {
      const cls = o.cls ?? meta.cls;
      if (o.refused) return { code: 2, commitMs: o.commitMs, waited: 0, staged, cls, text: String(o.out.value) };
      return { code: o.out.ok === false ? (o.out.value === "stale" ? 1 : 4) : 0, commitMs: o.commitMs, waited: o.waited ?? 0, staged, cls };
    };
    while (Date.now() < end) {
      const site = this.leader, stub = this.member(site);
      let c: CommitRes;
      // Sixth pass: the leader holds `commit` up to COMMIT_HOLD for the outcome, so the call's deadline is that plus
      // CALL_DEADLINE. Only a call that times out falls back to `outcomeRid`, which also tells whether it was admitted.
      try { c = await within(async () => (await stub.commit(this.group, rid, op, meta)) as CommitRes, COMMIT_HOLD + CALL_DEADLINE); }
      catch (e) {
        this.err(`commit ${site}: ${String(e)}`);
        if (!String(e).includes("deadline")) this.broken(site);
        if (missed("commit", null, site)) return gaveUp();
        if (String(e).includes("deadline")) {
          this.fallbacks++;
          const f = await within(async () => (await stub.outcomeRid(this.group, rid)) as OutcomeRes | { unknown: true }, AGENT_WAIT + 10_000)
            .catch((x) => { this.err(`outcome ${site}: ${String(x)}`); return null; });
          if (f && !("unknown" in f) && !("notLeader" in f)) return done(f);
        }
        await this.hedge(site); if (this.leader === site) this.follow(null);
        await sleep(100 * 2 ** (failures.commit ?? 0)); continue;
      }
      if ("notLeader" in c) {
        if (missed("commit", c.notLeader.leader, site)) return gaveUp();
        this.follow(c.notLeader.leader);
        if (!c.notLeader.leader) { await this.hedge(site); await sleep(100 * 2 ** (failures.commit ?? 0)); }
        continue;
      }
      if ("done" in c) return { code: c.done.ok === false ? (c.done.value === "stale" ? 1 : 4) : 0, commitMs: -1, waited: 0, staged, cls: meta.cls };
      this.hints = c.hints;
      if ("result" in c) return done(c.result);
      if ("refused" in c) return { code: 2, commitMs: -1, waited: 0, staged, cls: meta.cls, text: c.refused };
      if ("stage" in c) {
        if (!pack || staged) { this.err("stage without pack"); return { code: 3, commitMs: -1, waited: 0, staged, cls: meta.cls }; }
        await this.stage(pack); staged = true; op = unstaged(); meta = { ...meta, stored: pack.byteLength };
        continue;
      }
      let o: OutcomeRes;
      try { o = await within(async () => (await stub.outcome(this.group, c.admitted)) as OutcomeRes, AGENT_WAIT + 10_000); }
      catch (e) {
        this.err(`outcome ${site}: ${String(e)}`);
        if (!String(e).includes("deadline")) this.broken(site);
        if (missed("outcome", null, site)) return gaveUp();
        await sleep(100 * 2 ** (failures.outcome ?? 0)); continue;
      }
      if ("notLeader" in o) { if (missed("outcome", o.notLeader.leader, site)) return gaveUp(); this.follow(o.notLeader.leader); continue; }
      return done(o);
    }
    return { code: 3, commitMs: -1, waited: 0, staged, cls: meta.cls };
  }

  /** A `fresh` read on the leader. */
  async read(key: string): Promise<boolean> {
    for (let i = 0; i <= COMMIT_RETRIES + 2 && Date.now() < this.stopAt; i++) {
      const site = this.leader;
      let r: ReadRes;
      try { r = await within(async () => (await this.member(site).read(this.group, key)) as ReadRes, CALL_DEADLINE * 2); }
      catch (e) {
        this.err(`read ${site}: ${String(e)}`); if (!String(e).includes("deadline")) this.broken(site);
        await this.hedge(site); if (this.leader === site) this.follow(null); await sleep(100 * 2 ** i); continue;
      }
      if ("out" in r) return true;
      if ("busy" in r) { await sleep(50 * 2 ** i); continue; }
      this.follow(r.notLeader.leader);
      if (!r.notLeader.leader) await sleep(100 * 2 ** i);
    }
    return false;
  }

  /** One feed poll (design/AGENT-LAYER.md, "Activity feed"): learners only while any learner answers, else followers
   *  this caller does not believe lead, at most three members tried in all; none answering is the client's 429 with
   *  `Retry-After`. The caller learns the leader from commit and read replies, as the edge does from `ReplyMeta`. */
  async poll(cursor: number, paths: string[] | null, learners: number[]): Promise<{ res: FeedRes | null; tried: string[] }> {
    const tried: string[] = [];
    const ask = async (name: string, stub: DurableObjectStub<RaftMember>) => {
      tried.push(name);
      try {
        const r = await within(async () => (await stub.feed(this.group, cursor, FEED_WAIT_MAX, paths)) as FeedRes, FEED_WAIT_MAX + 15_000);
        if ("notServing" in r && r.notServing === "leader" && !name.startsWith("l")) this.leader = name as SiteKey;
        return "cursor" in r ? r : null;
      } catch (e) { this.err(`feed: ${String(e)}`); return null; }
    };
    for (const l of [...learners].sort(() => Math.random() - 0.5).slice(0, 3)) {
      const r = await ask(`l${l}`, this.learner(l));
      if (r) return { res: r, tried };
    }
    for (const f of SITES.filter((s) => s !== this.leader).sort(() => Math.random() - 0.5)) {
      if (tried.length >= 3) break;
      const r = await ask(f, this.member(f));
      if (r) return { res: r, tried };
    }
    return { res: null, tried };
  }
}

/** A Durable Object load client, pinned by its location hint at creation, running one spec from its alarm. */
export class Load9 extends DurableObject<Env> {
  async begin(spec: Spec): Promise<void> {
    await this.ctx.storage.deleteAll();
    await this.ctx.storage.put("spec", spec);
    await this.ctx.storage.setAlarm(Math.max(Date.now(), spec.start - 15_000));
  }
  async result(): Promise<unknown> { return this.ctx.storage.get("result"); }
  async partial(): Promise<unknown> { return this.ctx.storage.get("partial"); }
  /** Ends a run: a retried or running alarm sees `stopped` and returns. A redeploy of this script then ends the running one. */
  async stop(): Promise<void> { await this.ctx.storage.put("stopped", true); await this.ctx.storage.deleteAlarm(); }

  async alarm(): Promise<void> {
    const spec = await this.ctx.storage.get<Spec>("spec");
    if (!spec || (await this.ctx.storage.get("result")) || (await this.ctx.storage.get("stopped"))) return;
    const colo = /^colo=(\w+)$/m.exec(await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text())?.[1] ?? "?";
    const c = new Caller(this.env, spec.group);
    if (spec.leaderSite) c.leader = spec.leaderSite;
    if (spec.leaderSite === "c") c.bucket = this.env.STAGE_C;
    const t0 = spec.start, end = t0 + spec.seconds * 1000, share = 1 / spec.clients;
    c.colo = `wp9-${colo}`;
    c.stopAt = end + AGENT_WAIT + 15_000;
    // In-flight operations, sampled once a second: a client whose own queue grows is a harness bottleneck, not the group's.
    const flight: number[] = [];
    let shed = 0;
    const rows: Row[] = [];
    const texts: Record<string, number> = {}, badShapes: string[] = [];
    const reads = { ref: [0, 0], api: [0, 0], agent: [0, 0], refMs: [] as number[], apiMs: [] as number[], agentMs: [] as number[] };
    const feed: Record<string, number> = {}, feedHeld: number[] = [], feed429At: number[] = [];
    let feedEvents = 0;
    const late: number[] = [];
    const inflight = new Set<Promise<void>>();
    const track = (p: Promise<void>) => { inflight.add(p); void p.finally(() => inflight.delete(p)); };
    const sampler = (async () => { while (Date.now() < end) { flight.push(inflight.size); await sleep(1_000); } })();
    // One random MiB; a pack is a slice of it with a unique first 8 bytes.
    const rnd = new Uint8Array(1 << 20);
    for (let o = 0; o < rnd.length; o += 65536) crypto.getRandomValues(rnd.subarray(o, o + 65536));
    let packSeq = 0;
    const packOf = (n: number) => { const p = rnd.slice(0, n); new DataView(p.buffer).setFloat64(0, Date.now() + ++packSeq / 1e6); return p; };
    const size = () => spec.pushFixed ?? (Math.random() < spec.bigShare ? spec.bigBytes : Math.round(spec.packMin + Math.random() * (spec.packMax - spec.packMin)));
    const record = async (op: RaftOp, meta: Meta, bytes: number, fast = false) => {
      const ts = Date.now();
      try {
        const r = await c.write(op, meta);
        rows.push([ts - t0, KIND[meta.kind], CLS.indexOf(r.cls), Date.now() - ts, r.code, r.commitMs, r.waited, r.staged ? 1 : 0, bytes, fast ? 1 : 0]);
        if (r.text) {
          const shape = r.text.replace(/\d+/g, "n").replace(/^ng \S+/, "ng <ref>");
          texts[shape] = (texts[shape] ?? 0) + 1;
          if (!REFUSAL_SHAPE.test(r.text) && badShapes.length < 20) badShapes.push(r.text);
        }
        return r;
      } catch (e) {
        rows.push([ts - t0, KIND[meta.kind], CLS.indexOf(meta.cls), Date.now() - ts, 3, -1, 0, 0, bytes, fast ? 1 : 0]);
        c.errors[`write threw: ${String(e).slice(0, 80)}`] = (c.errors[`write threw: ${String(e).slice(0, 80)}`] ?? 0) + 1;
        return null;
      }
    };
    /** Runs `fire` at Poisson times of `rate` a second until the end. A slot more than 5 s late is skipped and counted. */
    const poisson = async (rate: number, fire: () => Promise<unknown>) => {
      if (rate <= 0) return;
      let at = Math.max(Date.now(), t0) + (-Math.log(1 - Math.random()) * 1000) / rate;
      while (at < end) {
        if (at > Date.now()) await sleep(at - Date.now());
        if (Date.now() - at > 5_000) late.push(at - t0);
        else if (inflight.size > 400) shed++;
        else track(fire().then(() => undefined));
        at += (-Math.log(1 - Math.random()) * 1000) / rate;
      }
    };
    const loops: Promise<void>[] = [];
    // Agents: each pushes its own ref every pushEvery s, compare-and-set on its last tip, at a phase spread over the fleet.
    const tips = new Map<number, string | null>();
    const pusherSlot = spec.pushers.indexOf(spec.index), np = spec.pushers.length;
    const owned = pusherSlot < 0 ? [] : Array.from({ length: Math.ceil(spec.agents / np) }, (_, k) => k * np + pusherSlot).filter((a) => a < spec.agents);
    const alarmLateMs = Date.now() - (spec.start - 15_000);
    if (spec.pushEvery > 0) for (const a of owned) loops.push((async () => {
      let at = t0 + (a / spec.agents) * spec.pushEvery * 1000, n = 0;
      while (at < end) {
        if (at > Date.now()) await sleep(at - Date.now());
        if (Date.now() - at > 5_000) { late.push(at - t0); at += spec.pushEvery * 1000; continue; }
        const key = `refs/heads/agent/${a}/task`, value = `${spec.run}-${a}-${n++}`, bytes = size();
        const r = await record({ kind: "cas", key, expect: tips.get(a) ?? null, value, pack: packOf(bytes), p: `pkg/${a % 50}/` },
          { cls: "Agent", actor: `agent-${a}`, kind: "push" }, bytes);
        if (r?.code === 0) tips.set(a, value);
        at += spec.pushEvery * 1000;
      }
    })());
    let side = 0;
    const put = (key: string, meta: Meta, fast = false) => record({ kind: "put", key, value: `${spec.run}-${side++}-${"x".repeat(900)}` }, meta, 1024, fast);
    loops.push(poisson(spec.agentSide * share, () => {
      const a = Math.floor(Math.random() * spec.agents);
      return put(`pr/${a}/${["sync", "testmerge", "check", "task", "lease", "rest"][Math.floor(Math.random() * 6)]}`, { cls: "Agent", actor: `agent-${a}`, kind: "side" });
    }));
    loops.push(poisson(spec.human * share, () => {
      const u = Math.floor(Math.random() * 50);
      return put(`human/${u}`, { cls: "Human", actor: `user-${u}`, kind: "side" });
    }));
    loops.push(poisson(spec.bot * share, () => put(`bot/${Math.floor(Math.random() * 100)}`, { cls: "Bot", actor: `bot-${spec.index}`, kind: "side" })));
    loops.push(poisson(spec.refReads * share, async () => {
      const t = Date.now(), ok = await c.read(Math.random() < 0.5 ? "refs/heads/main" : `refs/heads/agent/${Math.floor(Math.random() * spec.agents)}/task`);
      reads.ref[ok ? 0 : 1]++; if (reads.refMs.length < 10_000) reads.refMs.push(Date.now() - t);
    }));
    loops.push(poisson(spec.agentReads * share, async () => {
      const t = Date.now(), ok = await c.read(`pr/${Math.floor(Math.random() * spec.agents)}/rest`);
      reads.agent[ok ? 0 : 1]++; if (reads.agentMs.length < 10_000) reads.agentMs.push(Date.now() - t);
    }));
    loops.push(poisson(spec.apiReads * share, async () => {
      const t = Date.now(), ok = await c.read(`pr/${Math.floor(Math.random() * spec.agents)}/sync`);
      reads.api[ok ? 0 : 1]++; if (reads.apiMs.length < 20_000) reads.apiMs.push(Date.now() - t);
    }));
    // Third pass: Human pushes up to 1 MiB from the pusher clients, each user its own ref.
    const bigSize = () => Math.round((spec.bigMin ?? 0) + Math.random() * ((1 << 20) - (spec.bigMin ?? 0)));
    if (spec.humanPushes && pusherSlot >= 0) {
      const htips = new Map<string, string | null>();
      let hn = 0;
      loops.push(poisson(spec.humanPushes / np, async () => {
        const key = `refs/heads/human/${spec.index}-${Math.floor(Math.random() * 20)}/topic`, value = `${spec.run}-h${spec.index}-${hn++}`, bytes = bigSize();
        const r = await record({ kind: "cas", key, expect: htips.get(key) ?? null, value, pack: packOf(bytes), p: "human/" },
          { cls: "Human", actor: `user-push-${spec.index}`, kind: "push" }, bytes);
        if (r?.code === 0) htips.set(key, value);
      }));
    }
    // Merge queue: client 0 only, chained by parent tip on refs/heads/main.
    // Each merge names the tip the one before it will leave (`QueueMerge` chained by parent tip), so merges queued
    // together still apply in order; a failed or refused merge resets the chain to the last committed tip.
    let mainTip: string | null = null, issued: string | null = null, merges = 0;
    const merge = async () => {
      const value = `${spec.run}-main-${merges++}`, expect = issued;
      issued = value;
      const mb = spec.humanPushes ? bigSize() : 4096;
      const r = await record({ kind: "cas", key: "refs/heads/main", expect, value, pack: packOf(mb), p: "main/" },
        { cls: "MergeQueue", actor: "merge-queue", kind: "merge" }, mb);
      if (r?.code === 0) mainTip = value; else if (issued === value) issued = mainTip;
    };
    if (spec.index === spec.pushers[0] && spec.mergesPerMin > 0) loops.push((async () => {
      for (let at = t0 + 2_000; at < end; at += 60_000 / spec.mergesPerMin) {
        if (at > Date.now()) await sleep(at - Date.now());
        await merge(); // chained: the next merge's parent is this one's tip
      }
    })());
    // The fast user token, classed Human by the edge; the leader should class it Agent past DEMOTE_PER_MIN.
    if (spec.fastUser && spec.index === Math.min(1, spec.clients - 1)) loops.push((async () => {
      for (let at = t0; at < end; at += 60_000 / spec.fastUser!) {
        if (at > Date.now()) await sleep(at - Date.now());
        track(put(`user-fast/${Math.floor(Math.random() * 1000)}`, { cls: "Human", actor: "user-fast", kind: "side" }, true).then(() => undefined));
      }
    })());
    // Feed pollers: even ones send `paths` (one package), odd ones none; each re-polls at once with its new cursor.
    for (let p = spec.index; p < spec.pollers; p += spec.clients) loops.push((async () => {
      let cursor = -1;
      // 250 pollers send `paths` (one of 50 packages, each pushed by 10 agents), 250 send none (design/AGENT-LAYER.md,
      // journey A1 step 4).
      const paths = p % 2 === 0 ? [`pkg/${(p >> 1) % 50}/`] : null;
      // Pollers start at the run's start, spread over 10 s, once the learners have been fed.
      await sleep(Math.max(0, t0 - Date.now()) + Math.random() * 10_000);
      while (Date.now() < end - 2_000) {
        const { res, tried } = await c.poll(cursor, paths, spec.learners);
        if (!res) { feed["429"] = (feed["429"] ?? 0) + 1; if (feed429At.length < 300) feed429At.push(Date.now() - t0); await sleep(10_000); continue; }
        if ("cursor" in res) {
          const k = `ok:${tried[tried.length - 1].startsWith("l") ? "learner" : "follower"}:${res.site}${tried.length > 1 ? `:try${tried.length}` : ""}`;
          feed[k] = (feed[k] ?? 0) + 1;
          feedEvents += res.events; cursor = res.cursor;
          if (feedHeld.length < 5_000) feedHeld.push(res.heldMs);
        }
      }
    })());
    // The burst: this client's share of `pushes` pushes over 1 s, from agents of its own, plus merges from client 0.
    if (spec.burst && pusherSlot >= 0) {
      const b = spec.burst, mine = Math.floor(b.pushes / np) + (pusherSlot < b.pushes % np ? 1 : 0);
      loops.push((async () => {
        const at = t0 + b.at * 1000;
        if (at > Date.now()) await sleep(at - Date.now());
        const ps: Promise<unknown>[] = [];
        for (let k = 0; k < mine; k++) {
          ps.push((async () => {
            await sleep((k / mine) * 1000 + Math.random() * (1000 / mine));
            const a = `burst-${spec.index}-${k}`;
            await record({ kind: "cas", key: `refs/heads/agent/${a}/rebase`, expect: null, value: `${spec.run}-${a}`, pack: packOf(b.bytes), p: `pkg/${k % 100}/` },
              { cls: "Agent", actor: `agent-${a}`, kind: "push" }, b.bytes);
          })());
        }
        if (spec.index === spec.pushers[0]) for (let m = 0; m < b.merges; m++) { await sleep(m ? 900 / b.merges : 100); ps.push(merge()); }
        await Promise.all(ps);
      })());
    }
    // Saves a snapshot every 30 s, so a client that dies mid-run still reports what it did.
    const save = async (done: boolean) => this.ctx.storage.put(done ? "result" : "partial", {
      colo, index: spec.index, done, alarmLateMs, owned: owned.length, rows, texts, badShapes, reads, feed, feedHeld, feed429At, feedEvents, late, errors: c.errors,
      suspectsSent: c.suspectsSent, notLeaders: c.notLeaders, gaveUp: c.gaveUp, fallbacks: c.fallbacks, stageMs: c.stageMs, stages: c.stages, merges, flight, shed,
      firstErr: Object.fromEntries(Object.entries(c.firstErr).map(([k, t]) => [k, t - t0])),
    });
    const saver = (async () => { while (Date.now() < end) { await sleep(30_000); await save(false); } })();
    await Promise.all(loops);
    await Promise.all([...inflight]);
    await saver.catch(() => undefined);
    await sampler;
    await save(true);
  }
}

const json = (v: unknown, status = 200) => Response.json(v, { status });

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url);
    const body = req.method === "POST" ? await req.json<Record<string, any>>() : {};
    const group = String(body.group ?? "g0");
    const c = new Caller(env, group);
    try {
      switch (url.pathname) {
        case "/op": return json(await c.write(body.op, body.meta ?? { cls: "Human", actor: "runner", kind: "side" }));
        case "/status": return json(await Promise.all(SITES.map((s) => within(() => c.member(s).status(), 10_000).catch((e) => ({ site: s, error: String(e) })))));
        case "/transfer": {
          const st = await Promise.all(SITES.map((s) => within(() => c.member(s).status(), 10_000).catch(() => null)));
          const leader = st.find((x) => x?.role === "leader");
          if (!leader) return json({ error: "no leader" }, 409);
          return json({ from: leader.site, result: await c.member(leader.site).transfer(group, body.to, false) });
        }
        case "/learners": {
          await Promise.all(SITES.map((s) => c.member(s).setLearners(group, body.slots)));
          return json({ ok: true });
        }
        case "/log": return json(await Promise.all(SITES.map((s) => within(() => c.member(s).stats(false), 20_000)
          .then((r) => ({ site: s, role: r.role, log: r.log, entryMs: r.entryMs.slice(-50), appendMs: r.appendMs, bytesOut: r.bytesOut.slice(-50) }), (e) => ({ site: s, error: String(e) })))));
        case "/feedtest": return json(await c.learner(Number(body.slot)).feed(group, Number(body.cursor), 0, body.paths ?? null));
        case "/w9stats": {
          const voters = await Promise.all(SITES.map((s) => within(() => c.member(s).w9stats(!!body.reset), 20_000).catch((e) => ({ site: s, error: String(e) }))));
          const learners = await Promise.all(((body.learners ?? []) as number[]).map((l) => within(() => c.learner(l).w9stats(!!body.reset), 20_000)
            .then((r) => ({ slot: l, ...(r as Record<string, unknown>) }), (e) => ({ slot: l, error: String(e) }))));
          return json({ voters, learners });
        }
        case "/start": {
          // `hints`: location hints for the clients, in rotation. Each client gets the spec with its own index.
          const spec = body.spec as Spec, hints = body.hints as DurableObjectLocationHint[];
          const placed = await Promise.all(Array.from({ length: spec.clients }, async (_, i) => {
            const stub = env.LOAD.get(env.LOAD.idFromName(`${spec.run}-${i}`), { locationHint: hints[i % hints.length] });
            await stub.begin({ ...spec, index: i });
            return i;
          }));
          return json({ started: placed.length });
        }
        case "/purge": {
          // Empties a staging bucket's objects for one group prefix, 1,000 a call (`bucket: "c"` for the apac one).
          const b = body.bucket === "c" ? env.STAGE_C : env.STAGE;
          const l = await b.list({ prefix: String(body.prefix ?? ""), limit: 1000 });
          if (l.objects.length) await b.delete(l.objects.map((o) => o.key));
          return json({ deleted: l.objects.length, more: l.truncated });
        }
        case "/stop": {
          await Promise.all(Array.from({ length: Number(body.clients) }, (_, i) => env.LOAD.get(env.LOAD.idFromName(`${body.run}-${i}`)).stop()));
          return json({ stopped: Number(body.clients) });
        }
        case "/result": {
          const stub = env.LOAD.get(env.LOAD.idFromName(`${body.run}-${body.i}`));
          return json((await stub.result()) ?? { pending: true, partial: await stub.partial() });
        }
      }
    } catch (e) {
      return json({ error: String(e) }, 500);
    }
    return json({ error: "not found" }, 404);
  },
};

