// M0-WP12 copy of src/wp9/load.ts (copied 2026-10-30), deployed as `graft-wp12-load`. It keeps M0-WP9's journey 9 load and
// adds, per spec: agent pushes as agent-layer commands with a lease per agent (`leases`), lease renewals, feed pollers that
// record when each event reached them (`feedLat`), and the `FaultControl` object (plan "wp12") the members read.
//
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
import { FaultControl, Faults, faultyStub } from "./faults.ts";
import { changedPath, ref as refOf, scope, who as whoOf, type Kind } from "./fleet.ts";
import type { Who, XCmd, XOut } from "./lease.ts";

export { FaultControl };
import {
  CALL_DEADLINE, COMMIT_RETRIES, AGENT_WAIT, FEED_BATCH_ALL, FEED_WAIT_MAX, INLINE_MAX_HOT, REFUSAL_SHAPE, SITES, hintFor, learnerName, memberName,
  type AdmitClass, type Hints, type Meta, type RaftOp, type SiteKey, variant,
} from "./types.ts";

interface Env {
  M0_TOKEN: string;
  MEMBER_A: DurableObjectNamespace<RaftMember>;
  MEMBER_B: DurableObjectNamespace<RaftMember>;
  MEMBER_C: DurableObjectNamespace<RaftMember>;
  LOAD: DurableObjectNamespace<Load9>;
  FAULTS: DurableObjectNamespace<FaultControl>;
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
  /** A burst at `at` s: `pushes` agent pushes of `bytes` each over 1 s from the whole fleet, plus `merges` merges. */
  burst?: { at: number; pushes: number; bytes: number; merges: number };
  /** M0-WP12: the fleet's actor kind. Agent pushes become agent-layer `Push` commands with changed paths, and with
   *  `leases` each agent takes an Exclusive lease on its own scope at its first push and renews it every `renewEvery` s. */
  kind?: Kind; leases?: boolean; renewEvery?: number; leaseTtl?: number;
  /** Merges go through `Merge` commands on PRs 1..mergePrs, reading core rows only. */
  mergePrs?: number;
  /** Feed pollers' `paths` are `pkg/<n>/**` globs; each ring row's leader `at` is compared with the poller's receipt. */
  feedGlobs?: boolean;
  /** Pollers that poll a single member slot (LF-12): `follower` sends polls only to followers, never learners. */
  pollTargets?: "learners" | "followers";
  /** Every poller sends `paths` (`pkg/<p mod agents>/**`), or the glob given. */
  allPaths?: boolean | string;
  /** Polls go only to this voter site (LF-13). */
  pollSite?: SiteKey;
  /** Pollers keep the ring indices they received, for the no-loss check. */
  seenIdx?: boolean;
  /** Agent side commands are metadata-only writes (`Meta`), which the core paths never read (L0-16). */
  sideMeta?: boolean;
  /** An agent retries a failed push at once, up to this many attempts in all (L0-01). */
  pushAttempts?: number;
  /** Fork mode: agent pushes go to this group, whose `decide` reads the base group's rows as `deps`. */
  forkGroup?: string;
  /** L0-12 contention: `races` pairs of Graftyard-run tasks (r<k>a, r<k>b) hand an Exclusive lease on `race/<k>/**` back
   *  and forth: each holder takes it for 60 s and never renews, and the other retries every second until it lapses. */
  races?: number;
}

/** L0-13: renew lease `id` every `everyMs` until `forMs` after `granted`, probing it at each step. */
export interface RenewSpec { group: string; id: string; granted: number; holder: Who; prober: Who; probePath: string; probeFile: string; proberRef: string;
  everyMs: number; forMs: number }

/** One command row: [t ms from start, kind 0 push 1 merge 2 side, class 0 Human 1 MergeQueue 2 Bot 3 Agent, client ms,
 *  result 0 ok 1 stale 2 refused 3 error 4 other-false, leader commit ms (-1 if none), entries waited, staged 0/1, bytes,
 *  1 if from the fast user]. */
type Row = number[];
const KIND = { push: 0, merge: 1, side: 2 } as const;
const CLS: AdmitClass[] = ["Human", "MergeQueue", "Bot", "Agent"];
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const OP_DEADLINE = AGENT_WAIT + 30_000;

/** The `XOut` of an agent-layer command, which the member returns JSON-encoded in `Out.value`. */
const xo = (op: RaftOp, out: { value?: string | null }): { xout?: XOut } => (op.kind === "x" && out.value ? { xout: JSON.parse(out.value) as XOut } : {});

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
  /** Sixth pass: polls every tried member refused as lagging whose least lagged member was a voter, so the edge answered 429. */
  lastVoter = 0;
  notLeaders = 0;
  gaveUp = 0;
  errors: Record<string, number> = {};
  stageMs: number[] = [];
  /** The edge's member calls roll the group's fault plan too, labelled "edge><site>:<method>" (M0-WP12). */
  private faults: Faults;
  constructor(private env: Env, private group: string) { this.faults = new Faults(env.FAULTS, Math.random, `wp12:${group}`); }
  /** One stub per member, so calls through it keep their order (E-order); replaced after a call through it fails. */
  private stubs = new Map<SiteKey, DurableObjectStub<RaftMember>>();
  member(site: SiteKey): DurableObjectStub<RaftMember> {
    let s = this.stubs.get(site);
    if (!s) {
      const ns = this.env[`MEMBER_${site.toUpperCase()}` as "MEMBER_A"];
      s = faultyStub(ns.get(ns.idFromName(memberName(this.group, site)), { locationHint: hintFor(this.group, site) }), this.faults, `edge>${site}`);
      this.stubs.set(site, s);
    }
    return s;
  }
  private broken(site: SiteKey): void { this.stubs.delete(site); }
  learner(slot: number): DurableObjectStub<RaftMember> {
    const site = SITES[slot % 3], ns = this.env[`MEMBER_${site.toUpperCase()}` as "MEMBER_A"];
    return faultyStub(ns.get(ns.idFromName(learnerName(this.group, slot)), { locationHint: hintFor(this.group, site) }), this.faults, `edge>l${slot}`);
  }
  /** Errors by message and target, with the first time each was seen. */
  firstErr: Record<string, number> = {};
  private err(m: string): void { const k = m.slice(0, 90); this.errors[k] = (this.errors[k] ?? 0) + 1; this.firstErr[k] ??= Date.now(); }
  /** One hedge in flight per client: concurrent misses on the same leader share it. */
  private hedging: Promise<void> | null = null;
  /** `colo` labels this client's reports, as an edge's colo does. */
  colo = "wp12";
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

  /** One agent-layer command through `write`; returns the decoded `XOut` (or null on a give-up). */
  async x(cmd: XCmd, key: string, meta: Meta, pack?: Uint8Array): Promise<{ out: XOut | null; code: number; commitMs: number; cls: AdmitClass; text?: string }> {
    const r = await this.write({ kind: "x", key, cmd, ...(pack ? { pack } : {}) }, meta);
    return { out: r.xout ?? null, code: r.code, commitMs: r.commitMs, cls: r.cls, text: r.text };
  }

  async stage(pack: Uint8Array): Promise<void> {
    const t = Date.now();
    // `PackStore::place` fails in a "nop-" group (B0-31): no bucket accepts the pack.
    if (variant(this.group, "nop") !== null) throw new Error("place failed: every bucket refused");
    await (this.bucket ?? this.env.STAGE).put(`${this.group}/${crypto.randomUUID()}`, pack);
    if (this.stageMs.length < 20_000) this.stageMs.push(Date.now() - t);
  }

  /** One write. Returns the result code, the leader's admission-to-ack time, the entries waited, and whether it staged. */
  pollSite: SiteKey | null = null;
  /** Operations stop retrying at this time, so a client never keeps calling after its run. */
  stopAt = Infinity;
  async write(op: RaftOp, meta: Meta): Promise<{ code: number; commitMs: number; waited: number; staged: boolean; cls: AdmitClass; text?: string; xout?: XOut }> {
    const rid = crypto.randomUUID(), end = Math.min(this.stopAt, Date.now() + OP_DEADLINE);
    let staged = false;
    const pack = op.kind === "cas" || op.kind === "x" ? op.pack : undefined;
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
    while (Date.now() < end) {
      const site = this.leader, stub = this.member(site);
      let c: CommitRes;
      try { c = await within(async () => (await stub.commit(this.group, rid, op, meta)) as CommitRes, CALL_DEADLINE); }
      catch (e) {
        this.err(`commit ${site}: ${String(e)}`);
        if (!String(e).includes("deadline")) this.broken(site);
        if (missed("commit", null, site)) return gaveUp();
        await this.hedge(site); if (this.leader === site) this.follow(null);
        await sleep(100 * 2 ** (failures.commit ?? 0)); continue;
      }
      if ("notLeader" in c) {
        if (missed("commit", c.notLeader.leader, site)) return gaveUp();
        this.follow(c.notLeader.leader);
        if (!c.notLeader.leader) { await this.hedge(site); await sleep(100 * 2 ** (failures.commit ?? 0)); }
        continue;
      }
      if ("done" in c) return { code: c.done.ok === false ? (c.done.value === "stale" ? 1 : 4) : 0, commitMs: -1, waited: 0, staged, cls: meta.cls, ...xo(op, c.done) };
      this.hints = c.hints;
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
      const cls = o.cls ?? meta.cls;
      if (o.refused) return { code: 2, commitMs: o.commitMs, waited: 0, staged, cls, text: String(o.out.value) };
      const code = o.out.ok === false ? (o.out.value === "stale" ? 1 : 4) : 0;
      return { code, commitMs: o.commitMs, waited: o.waited ?? 0, staged, cls, ...xo(op, o.out) };
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
  async poll(cursor: number, paths: string[] | null, learners: number[]): Promise<{ res: FeedRes | null; tried: string[]; full: number }> {
    const tried: string[] = [];
    let full = 0;
    // Fifth pass, proposed edit 13: each tried member's lag when it refused as `lagging`, or null for any other answer.
    const lags: (number | null)[] = [];
    const stubs: DurableObjectStub<RaftMember>[] = [];
    const ask = async (name: string, stub: DurableObjectStub<RaftMember>, lastResort = false) => {
      tried.push(name);
      try {
        const r = await within(async () => (await stub.feed(this.group, cursor, FEED_WAIT_MAX, paths, lastResort)) as FeedRes, FEED_WAIT_MAX + 15_000);
        if ("notServing" in r && r.notServing === "leader" && !name.startsWith("l")) this.leader = name as SiteKey;
        if ("full" in r) full++;
        if (!lastResort) { stubs.push(stub); lags.push("notServing" in r && r.notServing === "lagging" ? (r.lagMs ?? 0) : null); }
        return "cursor" in r ? r : null;
      } catch (e) { this.err(`feed: ${String(e)}`); if (!lastResort) { stubs.push(stub); lags.push(null); } return null; }
    };
    // The operator's decision on fifth-pass edit 15: when every member tried refused as lagging, the edge holds the poll
    // on the least lagged of them only if that member is a learner; a lagging voter gets the 429 with `Retry-After`.
    const lastResort = async () => {
      // Outside the design: a group named "nolast-..." keeps the fourth pass's rule (429), as the fifth pass's control.
      if (variant(this.group, "nolast") !== null || !lags.length || lags.some((l) => l === null)) return null;
      const i = lags.indexOf(Math.min(...(lags as number[])));
      if (!tried[i].startsWith("l")) { this.lastVoter++; return null; }
      return ask(`${tried[i]}:last`, stubs[i], true);
    };
    // The edge tries up to three serving members in all: learners while any answers, then followers it does not
    // believe lead (design/AGENT-LAYER.md, "Activity feed").
    for (const l of [...learners].sort(() => Math.random() - 0.5).slice(0, 3)) {
      const r = await ask(`l${l}`, this.learner(l));
      // design/AGENT-LAYER.md, "Activity feed": the edge answers 429 at the first member that replies `full`.
      if (r || full) return { res: r, tried, full };
    }
    const sites = this.pollSite ? [this.pollSite] : SITES.filter((s) => s !== this.leader).sort(() => Math.random() - 0.5);
    for (const f of sites) {
      if (tried.length >= 3) break;
      const r = await ask(f, this.member(f));
      // design/AGENT-LAYER.md, "Activity feed": the edge answers 429 at the first member that replies `full`.
      if (r || full) return { res: r, tried, full };
    }
    return { res: await lastResort(), tried, full };
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

  /** L0-13's long renewal: one alarm per step, so no invocation runs past the alarm limit and the orb can sleep. */
  async beginRenew(r: RenewSpec): Promise<void> {
    await this.ctx.storage.deleteAll();
    await this.ctx.storage.put("renew", r);
    await this.ctx.storage.put("steps", []);
    await this.ctx.storage.setAlarm(Date.now() + 1_000);
  }
  async renewSteps(): Promise<unknown> { return { spec: await this.ctx.storage.get("renew"), steps: await this.ctx.storage.get("steps") }; }
  private async renewStep(r: RenewSpec): Promise<void> {
    const c = new Caller(this.env, r.group);
    const steps = (await this.ctx.storage.get<unknown[]>("steps")) ?? [];
    const now = Date.now(), age = now - r.granted;
    const rn = await c.x({ t: "Renew", who: r.holder, id: r.id, ttl: 3_600_000 }, "renew", { cls: "Agent", actor: "renewer", kind: "side" });
    // Probes: another task's overlapping Exclusive acquire (409 while Exclusive), an agent push into the scope (refused
    // under AgentsExclusive while Exclusive), and the lease's mode as `GET .../leases?paths=` reports it.
    const probe = await c.x({ t: "Acquire", who: r.prober, paths: [r.probePath], refs: [], mode: "Exclusive", ttl: 60_000 }, "probe", { cls: "Agent", actor: "prober", kind: "side" });
    if (probe.out?.ok && probe.out.lease) await c.x({ t: "Release", who: r.prober, ids: [probe.out.lease.id] }, "probe", { cls: "Agent", actor: "prober", kind: "side" });
    const push = await c.x({ t: "Push", who: r.prober, ref: `${r.proberRef}-${steps.length}`, expect: null, value: "v", changed: [r.probeFile], surface: "git" }, "probe",
      { cls: "Agent", actor: "prober", kind: "push" });
    let mode: unknown = null;
    for (let i = 0; i < 3 && mode === null; i++) {
      const rd = await c.member(c.leader).xread(r.group, "leases", [r.probePath]).catch(() => null) as Awaited<ReturnType<RaftMember["xread"]>> | null;
      if (rd && "out" in rd) mode = (rd.out as { id: string; mode: string; expires: number }[]).find((l) => l.id === r.id) ?? "absent";
      else if (rd && "notLeader" in rd) c.leader = rd.notLeader.leader ?? SITES[(SITES.indexOf(c.leader) + 1) % 3];
    }
    steps.push({ at: now, ageMin: +(age / 60_000).toFixed(1), renew: rn.out ? (rn.out.ok ? "ok" : rn.out.rule) : `none:${rn.code}`,
      probeAcquire: probe.out ? (probe.out.ok ? "ok" : `${probe.out.status}:${probe.out.rule}`) : `none:${probe.code}`,
      probePush: push.out ? (push.out.ok ? "ok" : push.out.rule) : `none:${push.code}`, mode });
    await this.ctx.storage.put("steps", steps);
    if (age < r.forMs) await this.ctx.storage.setAlarm(Date.now() + r.everyMs);
  }

  async alarm(): Promise<void> {
    const renew = await this.ctx.storage.get<RenewSpec>("renew");
    if (renew) return this.renewStep(renew);
    const spec = await this.ctx.storage.get<Spec>("spec");
    if (!spec || (await this.ctx.storage.get("result")) || (await this.ctx.storage.get("stopped"))) return;
    const colo = /^colo=(\w+)$/m.exec(await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text())?.[1] ?? "?";
    const c = new Caller(this.env, spec.group);
    c.pollSite = spec.pollSite ?? null;
    const cf = spec.forkGroup ? new Caller(this.env, spec.forkGroup) : c;
    if (spec.leaderSite) c.leader = spec.leaderSite;
    if (spec.leaderSite === "c") c.bucket = this.env.STAGE_C;
    const t0 = spec.start, end = t0 + spec.seconds * 1000, share = 1 / spec.clients;
    c.colo = `wp12-${colo}`;
    c.stopAt = end + AGENT_WAIT + 15_000;
    cf.stopAt = c.stopAt; cf.colo = `wp12-${colo}`;
    // In-flight operations, sampled once a second: a client whose own queue grows is a harness bottleneck, not the group's.
    const flight: number[] = [];
    let shed = 0;
    const rows: Row[] = [];
    const texts: Record<string, number> = {}, badShapes: string[] = [];
    const reads = { ref: [0, 0], api: [0, 0], agent: [0, 0], refMs: [] as number[], apiMs: [] as number[], agentMs: [] as number[] };
    const feed: Record<string, number> = {}, feedHeld: number[] = [], feed429At: number[] = [], feedLat: number[] = [];
    const seenBy: { g: string; s: number[] }[] = [], stoppedAt: number[][] = [], feedLatAt: number[] = [];
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
    // M0-WP12: per agent, its lease id and the rule counts of agent-layer replies.
    const leaseOf = new Map<number, string>(), xrules: Record<string, number> = {}, leaseLog: [number, number, string, number, number][] = [];
    const rule = (o: XOut | null) => { const k = !o ? "none" : o.ok ? "ok" : o.rule; xrules[k] = (xrules[k] ?? 0) + 1; };
    const kind = spec.kind;
    const acquire = async (a: number) => {
      const r = await c.x({ t: "Acquire", who: whoOf(kind!, a), paths: [scope(a)], refs: [], mode: "Exclusive", ttl: spec.leaseTtl, intent: `agent ${a}` },
        `lease/${a}`, { cls: "Agent", actor: `agent-${a}`, kind: "side" });
      rule(r.out);
      if (r.out?.ok && r.out.lease) { leaseOf.set(a, r.out.lease.id); leaseLog.push([Date.now() - t0, a, r.out.lease.id, r.out.lease.granted, r.out.lease.expires]); }
    };
    const alarmLateMs = Date.now() - (spec.start - 15_000);
    if (spec.pushEvery > 0) for (const a of owned) loops.push((async () => {
      let at = t0 + (a / spec.agents) * spec.pushEvery * 1000, n = 0;
      while (at < end) {
        if (at > Date.now()) await sleep(at - Date.now());
        if (Date.now() - at > 5_000) { late.push(at - t0); at += spec.pushEvery * 1000; continue; }
        const bytes = size();
        if (kind) {
          if (spec.leases && !leaseOf.has(a)) await acquire(a);
          const key = refOf(kind, a), value = `${spec.run}-${a}-${n}`, changed = [changedPath(a, n)];
          n++;
          const ts = Date.now();
          let r: Awaited<ReturnType<Caller["x"]>>, attempt = 0;
          // A failed push (a give-up or a 503 from `deps`) is retried at once by the agent, up to `pushAttempts` in all.
          for (;;) {
            attempt++;
            r = await cf.x({ t: "Push", who: whoOf(kind, a), ref: key, expect: tips.get(a) ?? null, value, changed, surface: "git" }, key,
              { cls: "Agent", actor: `agent-${a}`, kind: "push" }, packOf(bytes));
            const retry = !r.out || (!r.out.ok && r.out.status === 503);
            if (!retry || attempt >= (spec.pushAttempts ?? 1) || Date.now() > end) break;
          }
          rule(r.out);
          const ok = !!r.out?.ok;
          // [9] holds the attempts used, for L0-01's "within three attempts".
          rows.push([ts - t0, 0, CLS.indexOf(r.cls), Date.now() - ts, ok ? 0 : r.out?.ok === false && r.out.rule === "Stale" ? 1 : r.code === 3 ? 3 : r.out ? 2 : 3, r.commitMs, 0, 0, bytes, attempt]);
          if (ok) tips.set(a, value);
        } else {
          const key = `refs/heads/agent/${a}/task`, value = `${spec.run}-${a}-${n++}`;
          const r = await record({ kind: "cas", key, expect: tips.get(a) ?? null, value, pack: packOf(bytes), p: `pkg/${a % 50}/` },
            { cls: "Agent", actor: `agent-${a}`, kind: "push" }, bytes);
          if (r?.code === 0) tips.set(a, value);
        }
        at += spec.pushEvery * 1000;
      }
    })());
    const raceLog: number[][] = [];
    if (spec.races) for (let k = spec.index; k < spec.races; k += spec.clients) loops.push((async () => {
      const holders: Who[] = ["a", "b"].map((h) => ({ cred: { t: "Installation", app: 101, installation: 1001, task: `r${k}${h}` } }));
      let turn = k % 2, tries = 0;
      await sleep(Math.max(0, t0 - Date.now()) + Math.random() * 5_000);
      while (Date.now() < end) {
        const ts = Date.now();
        const r = await c.x({ t: "Acquire", who: holders[turn % 2], paths: [`race/${k}/**`], refs: [], mode: "Exclusive", ttl: 60_000 }, `race/${k}`,
          { cls: "Agent", actor: `racer-${k}-${turn % 2}`, kind: "side" });
        tries++;
        if (r.out?.ok && r.out.lease) { raceLog.push([ts - t0, k, turn % 2, r.out.lease.granted, r.out.lease.expires, tries]); turn++; tries = 0; }
        else await sleep(Math.max(0, 1_000 - (Date.now() - ts)));
      }
    })());
    // Lease renewals: each held lease renews every `renewEvery` s, as an agent's lease renewal side command.
    if (kind && spec.leases && spec.renewEvery) for (const a of owned) loops.push((async () => {
      let at = t0 + 30_000 + (a / spec.agents) * spec.renewEvery! * 1000;
      while (at < end) {
        if (at > Date.now()) await sleep(at - Date.now());
        const id = leaseOf.get(a);
        if (id) {
          const r = await c.x({ t: "Renew", who: whoOf(kind, a), id, ttl: spec.leaseTtl }, `lease/${a}`, { cls: "Agent", actor: `agent-${a}`, kind: "side" });
          rule(r.out);
          if (r.out && !r.out.ok && r.out.status === 404) { leaseOf.delete(a); leaseLog.push([Date.now() - t0, a, `${id}:lost`, 0, 0]); }
        }
        at += spec.renewEvery! * 1000;
      }
    })());
    let side = 0;
    const put = (key: string, meta: Meta, fast = false) => record({ kind: "put", key, value: `${spec.run}-${side++}-${"x".repeat(900)}` }, meta, 1024, fast);
    loops.push(poisson(spec.agentSide * share, () => {
      const a = Math.floor(Math.random() * spec.agents);
      if (spec.sideMeta) {
        const ts = Date.now();
        return c.x({ t: "Meta", kind: ["comment", "pr-body", "path_set", "summary"][Math.floor(Math.random() * 4)], body: "x".repeat(900) }, `meta/${a}`,
          { cls: "Agent", actor: `agent-${a}`, kind: "side" }).then((r) => { rule(r.out); rows.push([ts - t0, 2, 3, Date.now() - ts, r.out?.ok ? 0 : 3, r.commitMs, 0, 0, 1024, 0]); });
      }
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
    // Merge queue: client 0 only, chained by parent tip on refs/heads/main.
    // Each merge names the tip the one before it will leave (`QueueMerge` chained by parent tip), so merges queued
    // together still apply in order; a failed or refused merge resets the chain to the last committed tip.
    let mainTip: string | null = null, issued: string | null = null, merges = 0;
    const merge = async () => {
      const value = `${spec.run}-main-${merges++}`, expect = issued;
      issued = value;
      if (spec.mergePrs) {
        // A `Merge` of the next open PR: protection, review verdicts, and required checks are core rows.
        const ts = Date.now();
        const r = await c.x({ t: "Merge", who: { cred: { t: "User", user: 1 } }, pr: merges, ref: "refs/heads/main", expect, value }, "refs/heads/main",
          { cls: "MergeQueue", actor: "merge-queue", kind: "merge" }, packOf(4096));
        rule(r.out);
        const ok = !!r.out?.ok;
        rows.push([ts - t0, 1, 1, Date.now() - ts, ok ? 0 : r.out && !r.out.ok && r.out.rule === "Stale" ? 1 : r.out ? 2 : 3, r.commitMs, 0, 0, 4096, 0]);
        if (ok) mainTip = value; else if (issued === value) issued = mainTip;
        return;
      }
      const r = await record({ kind: "cas", key: "refs/heads/main", expect, value, pack: packOf(4096), p: "main/" },
        { cls: "MergeQueue", actor: "merge-queue", kind: "merge" }, 4096);
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
      const paths = spec.allPaths ? [typeof spec.allPaths === "string" ? spec.allPaths : `pkg/${p % spec.agents}/**`]
        : p % 2 === 0 ? [spec.feedGlobs ? `pkg/${(p >> 1) % spec.agents}/**` : `pkg/${(p >> 1) % 50}/`] : null;
      const seen: number[] = [];
      if (spec.seenIdx) seenBy.push({ g: paths?.[0] ?? "", s: seen });
      // Pollers start at the run's start, spread over 10 s, once the learners have been fed.
      await sleep(Math.max(0, t0 - Date.now()) + Math.random() * 10_000);
      while (Date.now() < end - 2_000) {
        const { res, tried, full } = await c.poll(cursor, paths, spec.pollTargets === "followers" ? [] : spec.learners);
        if (!res) {
          // The edge's 429: every tried member refused. `Retry-After` is FEED_BATCH_ALL when a member was full, and from
          // the tenth pass (ninth-pass edit 28, default; "old-" groups keep 2 s) when all refused as lagging too.
          const k = full ? "429:full" : "429";
          feed[k] = (feed[k] ?? 0) + 1; if (feed429At.length < 300) feed429At.push(Date.now() - t0);
          await sleep(full || variant(spec.group, "old") === null ? FEED_BATCH_ALL : 2_000); continue;
        }
        if ("cursor" in res) {
          const k = `ok:${tried[tried.length - 1].startsWith("l") ? "learner" : "follower"}:${res.site}${tried[tried.length - 1].endsWith(":last") ? ":last" : tried.length > 1 ? `:try${tried.length}` : ""}`;
          feed[k] = (feed[k] ?? 0) + 1;
          feedEvents += res.events; cursor = res.cursor;
          if (feedHeld.length < 5_000) feedHeld.push(res.heldMs);
          // Event latency: from the entry's leader `at` to this poller's receipt, on this client's clock.
          const got = Date.now();
          for (const [idx, at] of res.ev) { if (feedLat.length < 50_000 && at >= t0) { feedLat.push(got - at); feedLatAt.push(at - t0); } if (spec.seenIdx) seen.push(idx); }
          if (res.stopped && stoppedAt.length < 5_000) stoppedAt.push([got, res.heldMs, res.cursor]);
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
      feedLat, feedLatAt, xrules, leaseLog, raceLog, seenBy: done ? seenBy : [], stoppedAt,
      suspectsSent: c.suspectsSent, lastVoter: c.lastVoter, notLeaders: c.notLeaders, gaveUp: c.gaveUp, stageMs: c.stageMs, merges, flight, shed,
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
        // ---- M0-WP12 ----
        /** One agent-layer command through the write path. */
        case "/x": return json(await c.x(body.cmd, String(body.key ?? "x"), body.meta ?? { cls: "Human", actor: "runner", kind: "side" }));
        /** Many agent-layer commands at once, `conc` in flight; returns each `XOut`. */
        case "/xmany": {
          const cmds = body.cmds as { cmd: XCmd; key: string; meta?: Meta }[], conc = Number(body.conc ?? 20), out: unknown[] = new Array(cmds.length);
          let i = 0;
          await Promise.all(Array.from({ length: conc }, async () => {
            for (let k = i++; k < cmds.length; k = i++) {
              const t = Date.now();
              const r = await c.x(cmds[k].cmd, cmds[k].key, cmds[k].meta ?? { cls: "Human", actor: "runner", kind: "side" }).catch((e) => ({ out: null, code: 3, commitMs: -1, text: String(e) }));
              out[k] = { ...r, ms: Date.now() - t };
            }
          }));
          return json(out);
        }
        case "/mint": {
          for (let i = 0; i < 5; i++) {
            const r = await c.member(c.leader).mintToken(group, String(body.session)) as Awaited<ReturnType<RaftMember["mintToken"]>>;
            if (!("notLeader" in r)) return json(r);
            c.leader = r.notLeader.leader ?? SITES[(SITES.indexOf(c.leader) + 1) % 3];
          }
          return json({ error: "no leader" }, 503);
        }
        case "/xread": {
          for (let i = 0; i < 5; i++) {
            const r = await c.member(c.leader).xread(group, body.what, body.arg ?? []) as Awaited<ReturnType<RaftMember["xread"]>>;
            if (!("notLeader" in r)) return json(r);
            c.leader = r.notLeader.leader ?? SITES[(SITES.indexOf(c.leader) + 1) % 3];
          }
          return json({ error: "no leader" }, 503);
        }
        case "/renew": { await env.LOAD.get(env.LOAD.idFromName(`renew-${body.name}`), { locationHint: "enam" }).beginRenew(body.spec); return json({ ok: true }); }
        case "/renewsteps": return json(await env.LOAD.get(env.LOAD.idFromName(`renew-${body.name}`)).renewSteps());
        case "/leases": return json(await Promise.all(SITES.map((s) => within(() => c.member(s).leaseDump(group), 20_000).catch((e) => ({ site: s, error: String(e) })))));
        case "/clocks": {
          // Offsets between members, each measured by round trip from the asking member (F3), and the edge's own view.
          const probes = await Promise.all(SITES.map((s) => within(() => c.member(s).clockProbe(group, Number(body.n ?? 10)), 60_000).catch((e) => ({ site: s, error: String(e) }))));
          const edge: [string, number, number][] = [];
          for (let i = 0; i < Number(body.n ?? 10); i++) for (const s of SITES) {
            const t0 = Date.now(); const r = await within(() => c.member(s).clock(), 5_000).catch(() => null); const t1 = Date.now();
            if (r) edge.push([s, t1 - t0, r.now - (t0 + (t1 - t0) / 2)]);
          }
          return json({ probes, edge, edgeColo: /^colo=(\w+)$/m.exec(await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text())?.[1] });
        }
        case "/faults": { await env.FAULTS.getByName(`wp12:${group}`).set(body.plan); return json({ ok: true }); }
        case "/kill": return json(await c.member(body.site).kill(group).catch((e) => String(e)));
        case "/ringdump": return json(await c.member(body.site).ringDump(group, Number(body.from ?? 0)));
        case "/learnerstats": return json(await Promise.all(((body.learners ?? []) as number[]).map((l) => within(() => c.learner(l).w9stats(false), 20_000)
          .then((r) => ({ slot: l, ...(r as Record<string, unknown>) }), (e) => ({ slot: l, error: String(e) })))));
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

