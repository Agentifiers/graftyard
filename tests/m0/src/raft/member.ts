// M0-WP2 Raft member prototype (build/BUILD-PLAN.md, M0-WP2; design/DESIGN.md, Shape, "Chains", "Leadership",
// "The write path", "The read path"). One source, deployed as graft-raft-a (enam), graft-raft-b (weur), and
// graft-raft-c (apac), each binding the other two sites' member class with `script_name`. These are not the
// graft-site-* scripts, so deploying them never touches B0-15.
//
// Built here: the log with chained digests, `MemberState` persisted before any reply leaves (output gate), pre-vote,
// promises (`promise_until`), the leader lease with DRIFT, `led_term` resume, RESET_QUARANTINE, request-driven
// campaigns through `suspect` with the two-colo rule, group commit with one entry in flight, request-id dedupe,
// `verify` on followers, read-index, and transfer (completed or abandoned). Not built: `Joint` and `Degraded`
// configs, learners, `STEP_DOWN`, release-aware refusal handling, and snapshots. They belong to M0-WP3 and M0-WP4.
//
// The state machine is the M0 register (put, get, cas per key), so histories check with porcupine/.

import { DurableObject } from "cloudflare:workers";
import { Faults, faultyStub, type FaultControl, type FaultStats } from "../faults.ts";
import { sha256hex } from "./sha256.ts";
import {
  CALL_DEADLINE, COMMIT_RETRIES, DRIFT, ELECTION_AFTER, hintFor, LEARNER_GAP, LEASE, MODE_KEYS, mayLead, modesFrom, NO_MODES,
  POINTER_TTL, RELEASE_GROUP, RESET_QUARANTINE, SITES, STEP_DOWN, SUSPECT_COLOS, SUSPECT_WINDOW, memberName,
  type AppendReq, type AppendRes, type Body, type Entry, type Ev, type MemberStatus, type Modes, type Out, type RaftOp, type SiteKey,
  type Verdict, type VoteReq, type VoteRes, decideMode,
} from "./types.ts";
import { recordCall, type SiteHealth } from "./health.ts";
export { SiteHealth } from "./health.ts";

export interface MemberEnv {
  SITE: SiteKey;
  MEMBER_A: DurableObjectNamespace<RaftMember>;
  MEMBER_B: DurableObjectNamespace<RaftMember>;
  MEMBER_C: DurableObjectNamespace<RaftMember>;
  FAULTS?: DurableObjectNamespace<FaultControl>;
  /** The FaultControl object this deployment reads ("plan" when unset). */
  PLAN?: string;
  // M0-WP4, all optional so graft-raft-* behaves as before. RELEASE is this version's release number. READS is the
  // highest entry `format` it decodes and WRITES the format its leaders write. BREAK_KIND injects a defect:
  // "apply:<prefix>" throws while applying an event whose key starts with <prefix> (B0-05), and "refuse" refuses every
  // events entry in `verify` (B0-09).
  RELEASE?: string; READS?: string; WRITES?: string; BREAK_KIND?: string;
  /** This site's `SiteHealth` objects, one per target site (B0-07). */
  HEALTH?: DurableObjectNamespace<SiteHealth>;
}

/** `boot` names the instance that issued the ticket (design/DESIGN.md, `Ticket`): a reset leader that resumes `led_term`
 *  restarts `seq` and must not answer a ticket from before its reset. */
export type Ticket = { term: number; boot: number; seq: number };
export type CommitRes = { admitted: Ticket } | { done: Out } | { notLeader: { term: number; leader: SiteKey | null } };
export type OutcomeRes = { out: Out; commitMs: number; entry: number } | { notLeader: { term: number; leader: SiteKey | null } };
export type IndexRes = { index: number; entries: Entry[] } | { notLeader: { term: number; leader: SiteKey | null } };
export type ReadRes = { out: Out; viaRound: boolean } | { notLeader: { term: number; leader: SiteKey | null } } | { busy: string };

const ZERO = "0".repeat(64);
const BATCH_MAX = 2_000; // commands per entry; 1 KB commands keep an entry under the design's 1.5 MB
const CATCHUP_MAX = 200; // entries per AppendEntries
const digestOf = (e: Omit<Entry, "digest">) => sha256hex(`${e.prev}|${e.term}|${e.index}|${e.at}|${JSON.stringify(e.body)}`);
const BOOT_BODY: Body = { kind: "config", voters: SITES };
const BOOT: Entry = { term: 1, index: 1, prev: ZERO, at: 0, body: BOOT_BODY, digest: digestOf({ term: 1, index: 1, prev: ZERO, at: 0, body: BOOT_BODY }) };
/** A boot more than this long after the member last saved its state is an idle eviction, not a reset. Milestone 0
 *  measured idle members evicted after about 10 s, so this sits below that. */
const BOOT_IDLE = 5_000;
/** The drain alarm (M0-WP4, B0-07): a leader rechecks its site's mode this often while it has served a request within
 *  DRAIN_IDLE, so a leader on a drained site hands off without waiting for traffic. An idle group's last alarm wakes
 *  its leader once and arms no other, so idle groups stay unbilled after DRAIN_IDLE. */
const DRAIN_CHECK = 2_000;
const DRAIN_IDLE = 60_000;
const LINK_PINGS = 20; // pings in a `linkErrors` probe; one failure in 20 is the 5% threshold
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** One call with CALL_DEADLINE, retried at most `retries` times (design/DESIGN.md, "Commit and read paths"). */
export async function call<T>(fn: () => Promise<T>, deadline = CALL_DEADLINE, retries = COMMIT_RETRIES): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    let timer: ReturnType<typeof setTimeout> | null = null;
    try {
      return await Promise.race([fn(), new Promise<never>((_, rej) => { timer = setTimeout(() => rej(new Error("call deadline")), deadline); })]);
    } catch (e) {
      if (attempt >= retries) throw e;
    } finally {
      if (timer !== null) clearTimeout(timer);
    }
  }
}

let isolateFaults: Faults | undefined;
/** The isolate's fault plan reader. `Faults` refreshes its plan in the background and starts no new refresh while one is
 *  open; a refresh begun by an object that then reset never settles, and the isolate kept its old plan for the rest of
 *  the run (`raft-chaos-wp4-cf-4`: site C never saw the slow-storage rule or the paused call faults). So the reader is
 *  replaced when its plan is more than 3 s old. Test harness only; the shared `src/faults.ts` is unchanged. */
let isolateFaultsAt = 0;
/** Per-isolate fault counters that outlive reader swaps, so `/stats` shows which isolates injected (F36). `stale`
 *  counts swaps of a reader whose plan was over 3 s old. */
let isolateId = "";
let isolateStarted = 0;
const faultTotals: FaultStats & { stale: number } = { calls: 0, writes: 0, injected: {}, stale: 0 };
function foldStats(f: Faults): void {
  faultTotals.calls += f.stats.calls; faultTotals.writes += f.stats.writes;
  for (const [k, v] of Object.entries(f.stats.injected)) faultTotals.injected[k] = (faultTotals.injected[k] ?? 0) + v;
}
function faultsFor(env: MemberEnv): Faults {
  const fetchedAt = (isolateFaults as unknown as { fetchedAt?: number } | undefined)?.fetchedAt || isolateFaultsAt;
  if (!isolateFaults || Date.now() - fetchedAt > 3_000) {
    if (isolateFaults) { foldStats(isolateFaults); faultTotals.stale++; }
    isolateFaults = new Faults(env.FAULTS, Math.random, env.PLAN ?? "plan"); isolateFaultsAt = Date.now();
  }
  return isolateFaults;
}
/** This isolate's fault counters: the folded totals plus the live reader's. */
function isolateFaultStats(): { isolate: string; startedAt: number; calls: number; writes: number; injected: Record<string, number>; stale: number } {
  const live = isolateFaults?.stats ?? { calls: 0, writes: 0, injected: {} };
  const injected = { ...faultTotals.injected };
  for (const [k, v] of Object.entries(live.injected)) injected[k] = (injected[k] ?? 0) + v;
  if (!isolateId) { isolateId = crypto.randomUUID().slice(0, 8); isolateStarted = isolateFaultsAt; }
  return { isolate: isolateId, startedAt: isolateStarted, calls: faultTotals.calls + live.calls, writes: faultTotals.writes + live.writes, injected, stale: faultTotals.stale };
}
/** The `Release` view this isolate last read: refreshed from the own site's `release` member at most every
 *  POINTER_TTL, never moved backward, and kept when a refresh fails (design/DESIGN.md, FX-8). */
let modesView: { at: number; applied: number; modes: Modes } | null = null;
let modesRefresh: Promise<void> | null = null;

type Waiter = { resolve: (v: OutcomeRes) => void; promise: Promise<OutcomeRes>; result?: OutcomeRes; at: number };
type Queued = { rid: string; op: RaftOp; seq: number };

export class RaftMember extends DurableObject<MemberEnv> {
  private sql = this.ctx.storage.sql;
  private faults: Faults;
  private self: SiteKey;
  private group = "";

  // MemberState, persisted by save() before any reply leaves.
  private term = 1;
  private votedFor: SiteKey | null = null;
  private commitIdx = 0;
  private applied = 0;
  private promiseUntil = 0;
  private ledTerm: number | null = null;
  private transferAccepted: string | null = null;
  private boots: [number, number] = [0, 0];
  private steppedDownAt: number | null = null;

  // Volatile.
  private role: "leader" | "follower" | "candidate" = "follower";
  private leader: SiteKey | null = null;
  private last = 0;
  private lastTerm = 0;
  private lastDigest = ZERO;
  private lastLeaderSeen = 0;
  private campaigning = false;
  private backoffUntil = 0;
  private withholdUntil = 0;
  /** The leader's health window (`STEP_DOWN`): [time, ok] per commit-path member call and [time, ms] per own write
   *  confirmation. Cleared when leadership starts. */
  private callHealth: [number, SiteKey, boolean, string?][] = [];
  /** Followers that have answered an AppendEntries in this leadership. Failed calls to any other are not counted. */
  private heard = new Set<SiteKey>();
  private confirmHealth: [number, number][] = [];
  private steppingDown = false;
  /** Until when a held STEP_DOWN is not rechecked. */
  private healthHoldUntil = 0;
  private reports = new Map<string, number>();
  /** Sites whose leader two colos reported, until when. Unlike `withholdUntil`, this survives a term change, so a gray
   *  leader deposed by withheld promises cannot win back on the next `suspect` it receives as a follower. */
  private suspected = new Map<SiteKey, number>();
  /** Until when another voter refused this member's pre-vote as suspected; it does not campaign before then. */
  private suspectedUntil = 0;
  private refusals = 0;

  // M0-WP4: release-aware refusals (design/DESIGN.md, `verify` and `MemberState`).
  private readonly release: number;
  private readonly reads: number;
  private readonly writes: number;
  private readonly breakKind: string;
  /** Persisted. This member does not lead while it equals `release`; cleared at boot when the release changed. */
  private noLeadUntil: number | null = null;
  /** Persisted. Followers that refused an entry this member led, with the release they ran then. */
  private cannotFollow: Partial<Record<SiteKey, number>> = {};
  /** Refusals of one index that the leader has not classified yet. */
  private refusedBy = new Map<SiteKey, { index: number; release: number; why: string }>();
  private classifying = false;
  private applyErrors = 0;
  private refusedByMe = 0;
  private stepDowns = 0;
  private alarms: string[] = [];
  private raise(m: string): void { this.alarms.push(`${Date.now()} ${m}`); if (this.alarms.length > 50) this.alarms.shift(); this.event(`ALARM ${m}`); }
  private drainCheckAt = 0;
  /** When this instance last served a commit or read as leader, and the drain alarm it has armed (0 for none). */
  private lastServed = 0;
  private alarmAt = 0;
  private cfTried: Partial<Record<SiteKey, number>> = {};
  /** Persisted with `promise_until`: the leader the promise was given to. The promise bars votes for a rival only, so
   *  that leader may still win a new term after it is deposed (a reset leader that lost `led_term` to a higher term). */
  private promisedTo: SiteKey | null = null;

  // Leader state, reset by becomeLeader().
  private ready = false;
  private noopIndex = 0;
  private next: Partial<Record<SiteKey, number>> = {};
  private match: Partial<Record<SiteKey, number>> = {};
  private inflight: Partial<Record<SiteKey, boolean>> = {};
  private heartbeat: Partial<Record<SiteKey, boolean>> = {};
  private promisedAt: Partial<Record<SiteKey, number>> = {};
  /** Each follower's `stepped_down_at`, from its AppendEntries replies. */
  private peerSteppedDownAt: Partial<Record<SiteKey, number | null>> = {};
  private leaseEnd = 0;
  private roundWaiters: { since: number; resolve: (ok: boolean) => void }[] = [];
  private queue: Queued[] = [];
  /** Entries sent and not yet committed, oldest first. At most one, or `pipeDepth` for a pipelined group. */
  private pending: { index: number; started: number; seqs: number[]; outs: Map<number, Out> }[] = [];
  /** B0-01 experiment, outside the design: a group named "plN-..." keeps up to N entries in flight, each sent to each
   *  follower in its own AppendEntries call. Every other group keeps one entry in flight, as the design states. */
  private get pipeDepth(): number { return Number(/^pl(\d+)-/.exec(this.group)?.[1] ?? 1); }
  private tickets = new Map<number, Waiter>();
  private ridSeq = new Map<string, number>();
  private seq = 0;
  /** Random per instance, so tickets from before a reset never match this instance's (design/DESIGN.md, `Ticket`). */
  private readonly boot = crypto.getRandomValues(new Uint32Array(1))[0] * 2 ** 21 + (crypto.getRandomValues(new Uint32Array(1))[0] >>> 11);
  private transferring: { to: SiteKey; id: string } | null = null;
  /** Followers that acked an AppendEntries carrying the current transfer's id, so they will vote for its target. */
  private transferAcks = new Set<SiteKey>();
  // The leader's entries not yet in SQLite, and whether a flush is scheduled. The leader sends AppendEntries before it
  // writes anything, because the output gate holds every message sent after an unconfirmed write: writing first would
  // add the leader's own storage confirmation (about 25 ms) to every round. No message that implies a commit can leave
  // before the leader's write of that entry is confirmed, because the gate holds those.
  private unpersisted = new Map<number, Entry>();
  private flushScheduled = false;
  /** Leader-side timeline of the last 2,000 entries: [index, created, sent b, acked b, sent c, acked c, committed]. */
  private timeline = new Map<number, number[]>();
  // Admission to quorum ack per command, and commands per entry, since the last stats() reset (B0-01, B0-02).
  private commitLat: number[] = [];
  /** Leadership events with epoch-ms times, for RTO runs (`stats`). */
  private log: string[] = [];
  private slowNotes = 0;
  /** Requests this object received since the last stats reset, by method (B0-18 counts the leader's). */
  private reqs: Record<string, number> = {};
  private reqsSince = Date.now();
  private count(m: string): void { this.reqs[m] = (this.reqs[m] ?? 0) + 1; }
  // Learner side: the leader it asks, its batched read-index waiters, and its own read latency.
  private learnerOf: SiteKey = "a";
  private riBusy = false;
  private riLast = 0;
  private riWaiters: { since: number; resolve: (index: number | null) => void }[] = [];
  private learnerLat: number[] = [];
  private firstAcks = 40; // becomeLeader sets 0, so a new term logs its first acks
  private event(m: string): void { this.log.push(`${Date.now()} ${m}`); if (this.log.length > 300) this.log.splice(0, 100); }
  private entrySizes: number[] = [];
  private maxPending = 0; // most entries in flight at once since the last stats reset
  private entryMs: number[] = []; // entry stored on the leader to quorum ack: one replication round
  private appendMs: Partial<Record<SiteKey, number[]>> = {}; // AppendEntries call time per follower
  private serveMs: Partial<Record<SiteKey, number[]>> = {}; // the follower's own time inside append
  private bytesOut: number[] = []; // AppendEntries payload bytes
  private confirmSplit: [number, number][] = []; // follower: [cpu ms, confirm ms] per append

  constructor(ctx: DurableObjectState, env: MemberEnv) {
    super(ctx, env);
    this.self = env.SITE;
    this.faults = faultsFor(env);
    this.release = Number(env.RELEASE ?? 1); this.reads = Number(env.READS ?? 1); this.writes = Number(env.WRITES ?? 1);
    this.breakKind = env.BREAK_KIND ?? "";
    const sql = this.sql;
    sql.exec(`CREATE TABLE IF NOT EXISTS state (id INTEGER PRIMARY KEY CHECK (id = 1), grp TEXT, term INTEGER, voted_for TEXT,
              commit_idx INTEGER, applied INTEGER, promise_until INTEGER, led_term INTEGER, transfer TEXT, boots TEXT,
              stepped_down_at INTEGER)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS log (idx INTEGER PRIMARY KEY, term INTEGER NOT NULL, prev TEXT NOT NULL,
              digest TEXT NOT NULL, at INTEGER NOT NULL, body TEXT NOT NULL)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS req (rid TEXT PRIMARY KEY, idx INTEGER NOT NULL, out TEXT NOT NULL)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS refusal (id INTEGER PRIMARY KEY CHECK (id = 1), no_lead_until INTEGER, cannot_follow TEXT)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS promised (id INTEGER PRIMARY KEY CHECK (id = 1), leader TEXT)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS active (id INTEGER PRIMARY KEY CHECK (id = 1), at INTEGER)`);
    const row = sql.exec<Record<string, any>>(`SELECT * FROM state WHERE id = 1`).toArray()[0];
    if (row) {
      this.group = row.grp ?? ""; this.term = row.term; this.votedFor = row.voted_for; this.commitIdx = row.commit_idx;
      this.applied = row.applied; this.promiseUntil = row.promise_until; this.ledTerm = row.led_term;
      this.transferAccepted = row.transfer; this.boots = JSON.parse(row.boots); this.steppedDownAt = row.stepped_down_at;
    }
    const ref = sql.exec<{ no_lead_until: number | null; cannot_follow: string }>(`SELECT * FROM refusal WHERE id = 1`).toArray()[0];
    if (ref) { this.noLeadUntil = ref.no_lead_until; this.cannotFollow = JSON.parse(ref.cannot_follow); }
    this.promisedTo = (sql.exec<{ leader: SiteKey | null }>(`SELECT leader FROM promised WHERE id = 1`).toArray()[0]?.leader) ?? null;
    // `no_lead_until` clears when this member's release changes (design/DESIGN.md, `MemberState`).
    if (this.noLeadUntil !== null && this.noLeadUntil !== this.release) this.noLeadUntil = null;
    if (!sql.exec(`SELECT 1 FROM log WHERE idx = 1`).toArray().length) this.storeEntry(BOOT);
    const tail = sql.exec<{ idx: number; term: number; digest: string }>(`SELECT idx, term, digest FROM log ORDER BY idx DESC LIMIT 1`).one();
    this.last = tail.idx; this.lastTerm = tail.term; this.lastDigest = tail.digest;
    // A boot counts toward the flapping rule only if it interrupted work: one more than BOOT_IDLE after this member's
    // last saved state is the platform evicting an idle object, which request-driven groups see every minute or so.
    const lastActive = sql.exec<{ at: number }>(`SELECT at FROM active WHERE id = 1`).toArray()[0]?.at ?? 0;
    if (Date.now() - lastActive < BOOT_IDLE) this.boots = [this.boots[1], Date.now()];
    this.save();
  }

  // ---- persistence ----

  private save(): void {
    this.sql.exec(`INSERT OR REPLACE INTO state VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`, this.group, this.term, this.votedFor,
      this.commitIdx, this.applied, this.promiseUntil, this.ledTerm, this.transferAccepted, JSON.stringify(this.boots), this.steppedDownAt);
    this.sql.exec(`INSERT OR REPLACE INTO refusal VALUES (1, ?, ?)`, this.noLeadUntil, JSON.stringify(this.cannotFollow));
    this.sql.exec(`INSERT OR REPLACE INTO promised VALUES (1, ?)`, this.promisedTo);
    this.sql.exec(`INSERT OR REPLACE INTO active VALUES (1, ?)`, Date.now());
  }
  private storeEntry(e: Entry): void {
    this.sql.exec(`INSERT OR REPLACE INTO log VALUES (?, ?, ?, ?, ?, ?)`, e.index, e.term, e.prev, e.digest, e.at, JSON.stringify(e.body));
    this.last = e.index; this.lastTerm = e.term; this.lastDigest = e.digest;
  }
  private entry(i: number): Entry | undefined {
    const mem = this.unpersisted.get(i);
    if (mem) return mem;
    const r = this.sql.exec<{ idx: number; term: number; prev: string; digest: string; at: number; body: string }>(
      `SELECT * FROM log WHERE idx = ?`, i).toArray()[0];
    return r && { index: r.idx, term: r.term, prev: r.prev, digest: r.digest, at: r.at, body: JSON.parse(r.body) };
  }
  private logTerm(i: number): number | undefined {
    const mem = this.unpersisted.get(i);
    if (mem) return mem.term;
    return this.sql.exec<{ term: number }>(`SELECT term FROM log WHERE idx = ?`, i).toArray()[0]?.term;
  }
  private truncateFrom(i: number): void {
    if (i <= this.commitIdx) throw new Error(`refusing to truncate committed index ${i} (commit ${this.commitIdx})`);
    if (this.unpersisted.size) this.flush();
    this.sql.exec(`DELETE FROM log WHERE idx >= ?`, i);
    const tail = this.sql.exec<{ idx: number; term: number; digest: string }>(`SELECT idx, term, digest FROM log ORDER BY idx DESC LIMIT 1`).one();
    this.last = tail.idx; this.lastTerm = tail.term; this.lastDigest = tail.digest;
  }
  private kvGet(k: string): string | null {
    return this.sql.exec<{ v: string }>(`SELECT v FROM kv WHERE k = ?`, k).toArray()[0]?.v ?? null;
  }
  /** The fold at `upTo`: applied state in SQLite, plus the events of entries after `applied` up to `upTo`. */
  private foldGet(k: string, upTo: number): string | null {
    for (let i = Math.min(upTo, this.last); i > this.applied; i--) {
      const b = this.entry(i)!.body;
      if (b.kind === "events") for (let j = b.events.length - 1; j >= 0; j--) if (b.events[j].k === k) return b.events[j].v;
    }
    return this.kvGet(k);
  }
  /** A request id's committed result: from `req` once applied, else from a committed entry not applied yet. */
  private priorOut(rid: string): { out: Out; idx: number } | undefined {
    const row = this.sql.exec<{ out: string; idx: number }>(`SELECT out, idx FROM req WHERE rid = ?`, rid).toArray()[0];
    if (row) return { out: JSON.parse(row.out), idx: row.idx };
    for (let i = this.applied + 1; i <= this.commitIdx; i++) {
      const b = this.entry(i)?.body;
      const r = b?.kind === "events" ? b.requests.find((x) => x.rid === rid) : undefined;
      if (r) return { out: r.out, idx: i };
    }
    return undefined;
  }

  /** Leader only: queue an entry and send it; the write happens in `flush`, after the sends are on the wire. */
  private appendLocal(e: Entry): void {
    this.unpersisted.set(e.index, e);
    this.last = e.index; this.lastTerm = e.term; this.lastDigest = e.digest;
  }
  private scheduleFlush(): void {
    if (this.flushScheduled) return;
    this.flushScheduled = true;
    setTimeout(() => this.flush(), 0);
  }
  /** Writes the leader's queued entries, applies committed entries, and saves MemberState. */
  private flush(): void {
    this.flushScheduled = false;
    if (this.unpersisted.size) {
      if (this.role === "leader") {
        // Times this write's confirmation for `STEP_DOWN`. sync() resolves when the writes so far are confirmed.
        const t = Date.now(), term = this.term, n = this.unpersisted.size;
        void this.ctx.storage.sync().then(async () => {
          if (this.slowUntil > Date.now()) await this.gate();
          const ms = Date.now() - t;
          // A slow confirmation holds every message sent after the write (output gate): appends to both followers then
          // miss CALL_DEADLINE together (B0-04).
          if (ms > 500) this.event(`storage confirm took ${ms} ms for ${n} entries`);
          if (this.term === term) this.confirmed(ms);
        }, () => {});
      }
      this.write("entry");
      for (const e of [...this.unpersisted.values()].sort((x, y) => x.index - y.index)) {
        this.sql.exec(`INSERT OR REPLACE INTO log VALUES (?, ?, ?, ?, ?, ?)`, e.index, e.term, e.prev, e.digest, e.at, JSON.stringify(e.body));
      }
      this.unpersisted.clear();
    }
    if (this.applied < this.commitIdx) {
      this.write("apply");
      try { this.applyUpTo(this.commitIdx); } catch (e) { this.event(`apply failed: ${String(e).slice(0, 80)}`); }
    }
    this.save();
  }
  /** Applies committed entries in order: register writes and their request rows. */
  private applyUpTo(i: number): void {
    while (this.applied < Math.min(i, this.last)) {
      const e = this.entry(this.applied + 1)!;
      if (this.breakKind.startsWith("apply:") && e.body.kind === "events" && e.body.events.some((ev) => ev.k.startsWith(this.breakKind.slice(6)))) {
        this.applyErrors++;
        throw new Error(`m0 injected apply failure at ${e.index} (release ${this.release})`);
      }
      if (e.body.kind === "events") {
        for (const ev of e.body.events) this.sql.exec(`INSERT OR REPLACE INTO kv VALUES (?, ?)`, ev.k, ev.v);
        for (const r of e.body.requests) this.sql.exec(`INSERT OR IGNORE INTO req VALUES (?, ?, ?)`, r.rid, e.index, JSON.stringify(r.out));
      }
      this.applied = e.index;
    }
  }

  /** A storage write the fault plan may fail. A failed write resets the object, as an unconfirmed write does. Called
   *  right before each write, with no await between it and the write. */
  private write(what: string): void {
    this.faults = faultsFor(this.env);
    const rule = this.faults.rollNow("write", `${this.self}:${what}`);
    // A write rule with mode "slow-reply" stands in for gray storage (B0-06): the write is confirmed `hangMs` late, so
    // the output gate holds what this member sends after it until then (see `slowUntil`).
    if (rule?.mode === "slow-reply") { this.slowUntil = Math.max(this.slowUntil, Date.now() + (rule.hangMs ?? 1_000)); return; }
    if (rule) this.ctx.abort(`m0 injected fault: write ${this.self}:${what}`);
  }
  /** Until when an injected slow write is unconfirmed. Leader sends and follower acks wait for it, as the output gate
   *  holds messages sent after an unconfirmed write; votes and reads are not held in this stand-in. */
  private slowUntil = 0;
  private async gate(): Promise<void> { const wait = this.slowUntil - Date.now(); if (wait > 0) await sleep(wait); }
  private setGroup(group: string): void {
    if (!this.group) { this.group = group; this.save(); }
  }
  private peer(site: SiteKey): DurableObjectStub<RaftMember> {
    const ns = this.env[`MEMBER_${site.toUpperCase()}` as "MEMBER_A"];
    return faultyStub(ns.get(ns.idFromName(memberName(this.group, site)), { locationHint: hintFor(this.group, site) }), this.faults, `${this.self}>${site}`);
  }
  private isLeader(): boolean { return this.role === "leader"; }
  private others(): SiteKey[] { return SITES.filter((s) => s !== this.self); }

  // ---- terms and roles ----

  /** One commit-path member call by the leader: an AppendEntries that returned or failed. */
  private health(site: SiteKey, ok: boolean, err?: unknown): void {
    if (this.role !== "leader") return;
    // A follower not yet heard from in this leadership is one the quorum commits without, such as a killed leader still
    // resetting: its failures say nothing about this leader's site, and counting them moved new leaders 20 to 24 s after
    // a kill (M0-WP12; `tests/m0/results/m0-wp4.md`, "Leader kill").
    if (ok) this.heard.add(site); else if (!this.heard.has(site)) return;
    this.callHealth.push([Date.now(), site, ok, err === undefined ? undefined : /deadline/.test(String(err)) ? "deadline" : /InjectedFault/.test(String(err)) ? "injected" : "other"]);
    this.checkHealth();
  }
  private confirmed(ms: number): void {
    if (this.role !== "leader") return;
    this.confirmHealth.push([Date.now(), ms]);
    this.checkHealth();
  }
  /** `STEP_DOWN`: a leader sick by its own measure transfers to a caught-up follower and records `stepped_down_at`, so
   *  pre-vote keeps it from winning back for RESET_QUARANTINE. The error rate is judged per follower and must pass the
   *  threshold for every follower: a fault at the leader's own site breaks its calls to both, while a sick follower
   *  breaks only the calls to itself, and moving the leader would not help. Judged once each follower has 10 calls. */
  private checkHealth(): void {
    const since = Date.now() - STEP_DOWN.windowMs;
    this.callHealth = this.callHealth.filter(([t]) => t >= since);
    this.confirmHealth = this.confirmHealth.filter(([t]) => t >= since);
    if (this.steppingDown || this.transferring || this.role !== "leader" || !this.ready) return;
    const rates = this.others().map((f) => {
      const c = this.callHealth.filter(([, s]) => s === f);
      return c.length >= 10 ? c.filter(([, , ok]) => !ok).length / c.length : 0;
    });
    // The confirm rule trips only on at least `confirmSlowMin` confirmations over `confirmP99Ms` in the window
    // (design/DESIGN.md, `Health`): under 100 confirmations the p99 is the maximum, and one slow write moved leaders;
    // with 3, a healthy object's own periodic storage stalls moved them (F35).
    const confirms = this.confirmHealth.map(([, ms]) => ms).sort((a, b) => a - b);
    const p99 = confirms.length ? confirms[Math.floor(confirms.length * 0.99)] : 0;
    const slow = confirms.filter((ms) => ms > STEP_DOWN.confirmP99Ms).length;
    // Per follower: calls, failures by kind, and entries behind the leader's last, for the kill diagnosis.
    const detail = () => this.others().map((f) => {
      const c = this.callHealth.filter(([, s]) => s === f), k: Record<string, number> = {};
      for (const [, , ok, e] of c) if (!ok) k[e ?? "?"] = (k[e ?? "?"] ?? 0) + 1;
      return `${f}: ${c.length} calls ${JSON.stringify(k)} behind ${this.last - (this.match[f] ?? 0)}`;
    }).join("; ");
    // M0-WP4 variant for F37, groups named "min3-*" only: the commit-error rule also needs at least 3 failed calls to
    // every follower in the window, as the confirm rule needs `confirmSlowMin` slow confirmations.
    const failed = this.others().map((f) => this.callHealth.filter(([, s, ok]) => s === f && !ok).length);
    const enough = !this.group.startsWith("min3-") || Math.min(...failed) >= 3;
    const why = Math.min(...rates) > STEP_DOWN.commitErrors && enough ? `commit errors ${rates.map((r) => r.toFixed(2)).join("/")} [${detail()}]`
      : p99 > STEP_DOWN.confirmP99Ms && slow >= STEP_DOWN.confirmSlowMin ? `confirm p99 ${p99} ms, ${slow} of ${confirms.length} over ${STEP_DOWN.confirmP99Ms} ms` : null;
    if (!why || Date.now() < this.healthHoldUntil) return;
    // Transfer target (design/DESIGN.md, `Health`): a follower that may lead (undrained, able to follow), outside
    // RESET_QUARANTINE of its own `stepped_down_at`; on a confirm trip, one whose calls from this leader fail under
    // `commit_errors` in the window; on a commit-error trip, one whose calls from the other follower fail under
    // `commit_errors` by that follower's `linkErrors` probe, and a probe that cannot run disqualifies it, since the
    // leader's own calls to it are over the line too. Then the one whose calls failed least, then the most caught up;
    // hold if none qualifies. A confirm trip skips the probe. The target's own calls to the other follower were the
    // earlier reading; they measure the other follower too, and after an election left the fault on a follower they
    // qualified only the sick one (B0-06 `tgt-writes1`). Most caught up alone picked the sick follower when only its
    // storage failed (`b0-06-writes-wp4c3-writes1`); without the quarantine, two healthy sites handed a group back and
    // forth (F35).
    const m = modesView?.modes ?? NO_MODES;
    const quarantined = (s: SiteKey) => (this.peerSteppedDownAt[s] ?? 0) > Date.now() - RESET_QUARANTINE;
    const rate = (s: SiteKey) => rates[this.others().indexOf(s)];
    // The leader's-calls bar applies on a confirm trip only: a commit-error trip needs every follower over
    // `commit_errors` on the leader's calls, so there the bar would leave no target (M0-WP9 fifth pass, item 34).
    const errorsTrip = why.startsWith("commit errors");
    const may = (s: SiteKey) => mayLead(s, this.group, m) && this.cannotFollow[s] === undefined && !quarantined(s)
      && (errorsTrip || rate(s) <= STEP_DOWN.commitErrors);
    const candidates = this.others().filter(may);
    if (!candidates.length) {
      this.healthHoldUntil = Date.now() + STEP_DOWN.windowMs;
      const q = this.others().filter(quarantined);
      this.event(`STEP_DOWN held in term ${this.term}: ${why}, but no follower may lead${q.length ? ` (${q.join(", ")} stepped down within RESET_QUARANTINE)` : ""}`);
      return;
    }
    this.steppingDown = true;
    void (async () => {
      let ok = candidates, probe = "";
      if (errorsTrip) {
        const probes = await Promise.all(candidates.map(async (t) => {
          const other = this.others().find((s) => s !== t)!;
          return call(() => this.peer(other).linkErrors(this.group, t), 4 * CALL_DEADLINE + 500)
            .then((f) => ({ t, f, text: `${other}>${t} fails ${f.toFixed(2)}` }), (e) => ({ t, f: 1, text: `${other}>${t} probe failed: ${String(e).slice(0, 60)}` }));
        }));
        probe = `, ${probes.map((p) => p.text).join(", ")}`;
        ok = probes.filter((p) => p.f <= STEP_DOWN.commitErrors).map((p) => p.t);
      }
      ok.sort((x, y) => rate(x) - rate(y) || (this.match[y] ?? 0) - (this.match[x] ?? 0));
      if (!ok.length) {
        this.healthHoldUntil = Date.now() + STEP_DOWN.windowMs;
        this.event(`STEP_DOWN held in term ${this.term}: ${why}${probe}`);
        return;
      }
      const to = ok[0];
      this.event(`STEP_DOWN in term ${this.term}: ${why}${probe}, transferring to ${to}`);
      const r = await this.transfer(this.group, to, false);
      this.event(`STEP_DOWN transfer: ${r}`);
      if (this.role !== "leader") { this.steppedDownAt = Date.now(); this.stepDowns++; this.save(); }
    })().finally(() => { this.steppingDown = false; });
  }

  // ---- M0-WP4: site modes, drain, and refusals ----

  /** The site modes from the `Release` group's applied state, cached per isolate for POINTER_TTL and never moved
   *  backward. The first read in an isolate waits for it; later reads refresh in the background and keep the last view
   *  when every member fails. */
  private async modes(wait = false): Promise<Modes> {
    // The `Release` group's own members read the modes from their own fold, so it drains like any group.
    if (this.group === RELEASE_GROUP) return (await this.modeView()).modes;
    const stale = !modesView || Date.now() - modesView.at > POINTER_TTL;
    if (stale && !modesRefresh) {
      modesRefresh = (async () => {
        // Every member's applied state, highest applied wins: followers of a request-driven group learn a commit late.
        const views = await Promise.allSettled(SITES.map((s) => {
          const ns = this.env[`MEMBER_${s.toUpperCase()}` as "MEMBER_A"];
          const stub = faultyStub(ns.get(ns.idFromName(memberName(RELEASE_GROUP, s)), { locationHint: hintFor(RELEASE_GROUP, s) }), this.faults, `${this.self}>${s}`);
          return call(() => stub.modeView(), CALL_DEADLINE, 0);
        }));
        // The `release` leader applies an entry before its followers learn the commit, so the highest applied view is
        // the newest; a refresh is not held up by a slow or failing member past CALL_DEADLINE.
        const best = views.flatMap((v) => (v.status === "fulfilled" ? [v.value] : [])).sort((x, y) => y.applied - x.applied)[0];
        if (best && (!modesView || best.applied >= modesView.applied)) modesView = { at: Date.now(), applied: best.applied, modes: best.modes };
        // No newer view: keep this one and retry in 1 s rather than a full POINTER_TTL, so a refresh that failed on a
        // gray site's hung calls does not hold a drain back by another 5 s.
        else if (modesView) modesView.at = Date.now() - POINTER_TTL + 1_000;
      })().finally(() => { modesRefresh = null; });
    }
    // `wait`: the drain alarm, off the request path, waits for a stale view's refresh rather than judge the old one.
    if ((!modesView || wait) && modesRefresh) await modesRefresh;
    return modesView?.modes ?? NO_MODES;
  }
  /** Served by a `release` group member: the site modes in its applied state. */
  async modeView(): Promise<{ applied: number; modes: Modes }> {
    const v: Record<string, string | null> = {};
    for (const k of MODE_KEYS) v[k] = this.kvGet(k);
    return { applied: this.applied, modes: modesFrom(v) };
  }
  /** Why this member must not lead now: its site is drained or under the 1% lead limit, or `no_lead_until` holds. */
  private async leadBar(wait = false): Promise<string | null> {
    if (this.noLeadUntil === this.release) return `no_lead_until ${this.noLeadUntil}`;
    const m = await this.modes(wait);
    return mayLead(this.self, this.group, m) ? null : m.drained.includes(this.self) ? "drained" : "lead limit";
  }
  /** A leader that must not lead hands off within one lease to an undrained follower that can follow (design/DESIGN.md,
   *  `transfer` and `SiteMode`). Checked on requests, at most once a second, and by the drain alarm (`alarm`). */
  private async maybeHandOff(wait = false): Promise<void> {
    if (this.role !== "leader" || !this.ready || this.transferring || this.steppingDown || Date.now() < this.drainCheckAt) return;
    this.drainCheckAt = Date.now() + 1_000;
    const why = await this.leadBar(wait);
    if (!why || this.role !== "leader" || this.transferring) return;
    const m = await this.modes();
    const to = this.others().filter((s) => mayLead(s, this.group, m) && this.cannotFollow[s] === undefined)
      .sort((x, y) => (this.match[y] ?? 0) - (this.match[x] ?? 0))[0];
    if (!to) { this.event(`must not lead (${why}) but no follower can take over`); return; }
    this.event(`handing off (${why}) to ${to}`);
    const r = await this.transfer(this.group, to, false);
    this.event(`hand-off: ${r}`);
  }
  /** Arms the drain alarm if none is armed. Unconfirmed, so it never holds a reply behind the output gate; a lost alarm
   *  is re-armed by the next request this leader serves. */
  private armDrainCheck(): void {
    if (this.alarmAt) return;
    this.alarmAt = Date.now() + DRAIN_CHECK;
    void this.ctx.storage.setAlarm(this.alarmAt, { allowUnconfirmed: true });
  }
  /** The drain alarm: a leader, or an evicted leader still holding `led_term`, whose group wrote or was served within
   *  DRAIN_IDLE checks its site's mode, hands off if it must not lead, and re-arms. An evicted leader resumes `led_term`
   *  only to hand off; one that may lead waits for a request, as before. A follower stops. */
  async alarm(): Promise<void> {
    this.alarmAt = 0;
    const led = this.role === "leader" || this.ledTerm === this.term;
    if (!led || Date.now() - Math.max(this.lastServed, this.entry(this.last)?.at ?? 0) > DRAIN_IDLE) return;
    this.armDrainCheck();
    if (this.role !== "leader" && ((await this.leadBar(true)) === null || !(await this.resume()))) return;
    await this.maybeHandOff(true);
  }

  /** The leader's side of a refusal (design/DESIGN.md, `verify`). A refusal is classified once the refused entry's
   *  round has settled, because an older follower's refusal can arrive before the other follower's ack commits it:
   *  - committed, or refused by a same-or-newer follower that is not one of two own-release refusers: keep leading, add
   *    the refuser to `cannot_follow`, and alarm;
   *  - uncommitted and refused by an older follower: truncate the uncommitted tail, persist `no_lead_until`, transfer;
   *  - uncommitted and refused by both followers on this release: own defect, truncate the uncommitted tail, alarm. */
  private onRefusal(site: SiteKey, res: AppendRes): void {
    const index = res.refusedAt ?? this.next[site] ?? this.last, release = res.release ?? this.release;
    this.refusedBy.set(site, { index, release, why: res.refused ?? "" });
    if (this.classifying) return;
    this.classifying = true;
    const term = this.term;
    void (async () => {
      const other = this.others().find((s) => s !== site)!;
      const deadline = Date.now() + CALL_DEADLINE * (1 + COMMIT_RETRIES);
      while (Date.now() < deadline && this.role === "leader" && this.term === term && this.commitIdx < index
        && (this.refusedBy.get(other)?.index ?? 0) < index && this.cannotFollow[other] === undefined) await sleep(20);
      if (this.role !== "leader" || this.term !== term) return;
      const refusers = [...this.refusedBy.entries()].filter(([, r]) => r.index <= index);
      this.refusedBy.clear();
      const committed = this.commitIdx >= index;
      const older = refusers.filter(([, r]) => r.release < this.release);
      const own = refusers.filter(([, r]) => r.release === this.release);
      if (!committed && older.length) {
        this.raise(`older release ${older.map(([s, r]) => `${s}@${r.release}`).join(",")} refused uncommitted ${index}: ${older[0][1].why}`);
        this.dropUncommitted();
        this.noLeadUntil = this.release; this.save();
        this.drainCheckAt = 0;
        await this.maybeHandOff();
        return;
      }
      if (!committed && own.length === 2) {
        this.raise(`both followers on release ${this.release} refused uncommitted ${index}: own defect`);
        this.dropUncommitted();
        return;
      }
      for (const [s, r] of refusers) {
        if (this.cannotFollow[s] === r.release) continue;
        this.cannotFollow[s] = r.release;
        this.raise(`${s} on release ${r.release} cannot follow (${committed ? "committed" : "uncommitted"} ${index}): ${r.why}`);
      }
      this.save();
    })().finally(() => { this.classifying = false; });
  }
  /** Truncates the leader's uncommitted tail and answers its commands `NotLeader`, so the edge re-sends each with the
   *  same request id and the next leader decides it. */
  private dropUncommitted(): void {
    const nl: OutcomeRes = { notLeader: { term: this.term, leader: null } };
    for (const p of this.pending) for (const seq of p.seqs) {
      const w = this.tickets.get(seq);
      if (w && !w.result) { w.result = nl; w.resolve(nl); }
    }
    const seqs = new Set(this.pending.flatMap((p) => p.seqs));
    for (const [rid, seq] of this.ridSeq) if (seqs.has(seq)) this.ridSeq.delete(rid);
    this.pending = [];
    if (this.last > this.commitIdx) this.truncateFrom(this.commitIdx + 1);
    for (const s of this.others()) this.next[s] = Math.min(this.next[s] ?? this.last + 1, this.last + 1);
  }

  /** Share of LINK_PINGS pings from this member to `to` that fail or miss CALL_DEADLINE, five at a time. */
  async linkErrors(group: string, to: SiteKey): Promise<number> {
    this.setGroup(group);
    let failed = 0;
    for (let i = 0; i < LINK_PINGS; i += 5) {
      const r = await Promise.allSettled(Array.from({ length: 5 }, () => call(() => this.peer(to).ping(), CALL_DEADLINE, 0)));
      failed += r.filter((x) => x.status === "rejected").length;
    }
    return failed / LINK_PINGS;
  }

  private stepDownTo(term: number): void {
    if (this.unpersisted.size || this.flushScheduled) this.flush();
    if (term > this.term) { this.term = term; this.votedFor = null; this.ledTerm = null; this.withholdUntil = 0; this.reports.clear(); }
    this.role = "follower";
    this.endLeadership();
    this.save();
  }
  private endLeadership(): void {
    this.ready = false;
    this.transferring = null;
    for (const w of this.roundWaiters) w.resolve(false);
    this.roundWaiters = [];
    const nl: OutcomeRes = { notLeader: { term: this.term, leader: this.leader === this.self ? null : this.leader } };
    for (const [seq, w] of this.tickets) if (!w.result) { w.result = nl; w.resolve(nl); this.tickets.delete(seq); }
    this.queue = []; this.ridSeq.clear(); this.pending = [];
    if (this.leader === this.self) this.leader = null;
  }

  private becomeLeader(resumed: boolean): void {
    this.role = "leader"; this.leader = this.self; this.ledTerm = this.term; this.firstAcks = 0;
    this.callHealth = []; this.confirmHealth = []; this.healthHoldUntil = 0; this.heard.clear();
    this.ready = false; this.leaseEnd = 0; this.promisedAt = {}; this.pending = [];
    for (const s of this.others()) { this.next[s] = this.last + 1; this.match[s] = 0; this.heartbeat[s] = true; }
    if (!resumed) {
      // The first entry of every term. Committing it commits everything before it.
      const e = { term: this.term, index: this.last + 1, prev: this.lastDigest, at: Date.now(), body: { kind: "noop" } as Body };
      this.appendLocal({ ...e, digest: digestOf(e) });
      this.noopIndex = e.index;
    } else {
      this.noopIndex = this.sql.exec<{ i: number }>(`SELECT min(idx) AS i FROM log WHERE term = ?`, this.term).one().i ?? this.last;
    }
    for (const s of this.others()) void this.replicate(s);
    this.scheduleFlush();
  }

  /** Pre-vote, then a vote, for term + 1. `transfer` names a leadership transfer this member was sent. */
  private async campaign(transfer: string | null): Promise<Verdict> {
    if (this.role === "leader") return "leader";
    if (this.campaigning) return "busy";
    // A reset leader resumes `led_term` after one round rather than start a new term (design/DESIGN.md, `led_term`).
    if (!transfer && this.ledTerm === this.term) return (await this.resume()) ? "leader" : "lost";
    if (!transfer && Date.now() < this.promiseUntil) return "promised";
    if (!transfer && Date.now() < this.backoffUntil) return "busy";
    this.campaigning = true;
    let won = false;
    try {
      const startTerm = this.term;
      // A drained, lead-limited, or `no_lead_until` member refuses a transfer, and campaigns on its own only if pre-vote
      // finds no voter that can lead.
      const bar = await this.leadBar();
      if (transfer && bar) { this.event(`refused transfer: ${bar}`); return "lost"; }
      const cannotLead = !transfer && bar !== null;
      if (this.term !== startTerm || (this.role as string) !== "follower") return (this.role as string) === "leader" ? "leader" : "lost";
      const req = (): VoteReq => ({ term: startTerm + 1, candidate: this.self, lastIndex: this.last, lastTerm: this.lastTerm,
        transfer, boots: this.boots, steppedDownAt: this.steppedDownAt, cannotLead });
      if (!transfer) {
        const pre = await this.poll(req(), true);
        if (pre.higher > this.term) { this.stepDownTo(pre.higher); return "lost"; }
        if (!pre.granted && pre.suspected) {
          // Two colos reported this member as a gray leader: stand aside for the window instead of contending.
          this.suspectedUntil = this.backoffUntil = Date.now() + 2 * SUSPECT_WINDOW;
          this.event(`pre-vote for ${startTerm + 1} refused as suspected; not campaigning for ${2 * SUSPECT_WINDOW} ms`);
        }
        if (!pre.granted || this.term !== startTerm || (Date.now() < this.promiseUntil)) {
          this.event(`pre-vote lost for ${startTerm + 1}: granted ${pre.granted}, term ${this.term}, promise ${this.promiseUntil - Date.now()}`);
          return "lost";
        }
      }
      this.write("campaign");
      this.term = startTerm + 1; this.votedFor = this.self; this.ledTerm = null; this.role = "candidate"; this.leader = null;
      this.save();
      const votes = await this.poll(req(), false);
      if (votes.higher > this.term) { this.stepDownTo(votes.higher); return "lost"; }
      if (this.role !== "candidate" || this.term !== startTerm + 1) return "lost";
      if (!votes.granted) { this.role = "follower"; this.event(`vote lost for ${this.term}`); return "lost"; }
      this.event(`won term ${this.term}`);
      this.becomeLeader(false);
      won = true;
      return "leader";
    } finally {
      this.campaigning = false;
      // Two survivors hedged at once would split votes forever; the loser waits a random 100 to 400 ms.
      if (!won) this.backoffUntil = Math.max(this.backoffUntil, Date.now() + 100 + Math.random() * 300);
    }
  }

  /** Asks both other voters at once and returns at the first grant, which with this member's own vote is a majority of
   *  three, so a hung voter costs nothing. Otherwise it waits for both answers or their deadlines. */
  /** A candidate that cannot lead waits for both pre-vote answers and is granted only if no voter that answered can
   *  lead itself. */
  private poll(req: VoteReq, pre: boolean): Promise<{ granted: boolean; higher: number; suspected: boolean }> {
    const all = pre && !!req.cannotLead;
    return new Promise((resolve) => {
      let pending = 2, higher = 0, suspected = false, grants = 0, canLead = false;
      for (const s of this.others()) {
        call(() => this.peer(s).vote(this.group, req, pre)).then((r) => {
          higher = Math.max(higher, r.term);
          if (r.why === "suspected") suspected = true;
          if (r.why === "voter can lead") canLead = true;
          if (r.granted) { grants++; if (!all) resolve({ granted: true, higher, suspected }); }
        }, () => {}).finally(() => { if (--pending === 0) resolve({ granted: grants > 0 && !canLead, higher, suspected }); });
      }
    });
  }

  /** A restarted leader resumes `led_term` after one round, because no other member can lead that term. */
  private async resume(): Promise<boolean> {
    if (this.role === "leader" || this.ledTerm !== this.term) return this.role === "leader";
    this.becomeLeader(true);
    const ok = await this.round(Date.now());
    const role = this.role as string;
    if (!ok && role === "leader") { this.role = "follower"; this.endLeadership(); }
    if (this.isLeader() && this.commitIdx >= this.noopIndex) { this.ready = true; this.pump(); }
    return this.isLeader();
  }

  // ---- replication ----

  private async replicate(site: SiteKey): Promise<void> {
    if (this.inflight[site]) return;
    this.inflight[site] = true;
    const term = this.term;
    let failures = 0, failingSince = 0;
    try {
      while (this.role === "leader" && this.term === term) {
        // A site that cannot follow gets one attempt every 5 s, so the leader learns when its release changes. The loop
        // sleeps out the wait rather than stop, because nothing else restarts it while the follower is behind.
        if (this.cannotFollow[site] !== undefined) {
          const wait = 5_000 - (Date.now() - (this.cfTried[site] ?? 0));
          if (wait > 0) { await sleep(wait); continue; }
          this.cfTried[site] = Date.now();
        }
        const from = this.next[site]!;
        if (from > this.last && !this.heartbeat[site]) break;
        const entries: Entry[] = [];
        for (let i = from; i <= Math.min(this.last, from + CATCHUP_MAX - 1); i++) entries.push(this.entry(i)!);
        const prev = from - 1;
        const req: AppendReq = { term, leader: this.self, prevIndex: prev, prevTerm: this.logTerm(prev) ?? 0,
          prevDigest: prev === 0 ? ZERO : this.entry(prev)?.digest ?? ZERO, entries,
          commit: this.commitIdx, transfer: this.transferring?.id ?? null };
        if (this.slowUntil > Date.now()) { await this.gate(); if (this.role !== "leader" || this.term !== term) break; }
        const sentAt = Date.now();
        this.heartbeat[site] = false;
        const slot = site === this.others()[0] ? 1 : 3;
        for (const e of entries) { const t = this.timeline.get(e.index); if (t && !t[slot]) t[slot] = sentAt; }
        let res: AppendRes;
        try {
          res = await call(() => this.peer(site).append(this.group, req), CALL_DEADLINE, 0);
          this.health(site, true);
          recordCall(this.env.HEALTH, this.self, site, true);
          if (failures) this.event(`append to ${site} ok after ${failures} failures, ${Date.now() - sentAt} ms`);
          if (this.firstAcks < 40) { this.firstAcks++; this.event(`ack ${site} term ${term} entries ${entries.length} (${entries[0]?.index ?? "-"}..) ${Date.now() - sentAt} ms, serve ${res.serveMs} ms`); }
          failures = 0; failingSince = 0;
          if (entries.length) {
            for (const e of entries) { const t = this.timeline.get(e.index); if (t && !t[slot + 1]) t[slot + 1] = Date.now(); }
            (this.appendMs[site] ??= []).push(Date.now() - sentAt);
            (this.serveMs[site] ??= []).push(res.serveMs ?? -1);
            if (this.bytesOut.length < 5000) this.bytesOut.push(JSON.stringify(entries).length);
          }
        } catch (err) {
          this.health(site, false, err);
          recordCall(this.env.HEALTH, this.self, site, false);
          // The elapsed time tells a late timer (a busy isolate) from a call that missed its deadline (B0-04).
          if (failures === 0) this.event(`append to ${site} failed after ${Date.now() - sentAt} ms: ${String(err).slice(0, 60)}`);
          failures++;
          failingSince ||= sentAt;
          this.heartbeat[site] = true;
          // Retry COMMIT_RETRIES times at once, then keep trying while the round, or a transfer's catch-up, still needs
          // this follower: `transfer` only waits for `match`, so a loop that stopped here left it waiting out a LEASE.
          if (failures > COMMIT_RETRIES) {
            if (Date.now() - failingSince > 30_000 || (this.commitIdx >= this.last && !this.roundWaiters.length && this.transferring?.to !== site)) break;
            await sleep(250);
          }
          continue;
        }
        if (res.term > this.term) { this.stepDownTo(res.term); break; }
        if (this.role !== "leader" || this.term !== term) break;
        if (res.steppedDownAt !== undefined) this.peerSteppedDownAt[site] = res.steppedDownAt;
        // Refusals carry the refuser's release, and so do acks; the leader learns a release change from either.
        if (res.release !== undefined && this.cannotFollow[site] !== undefined && res.release !== this.cannotFollow[site]) {
          this.event(`${site} runs release ${res.release} now; it can follow again`);
          delete this.cannotFollow[site]; this.save();
        }
        if (res.ok) {
          if (req.transfer && req.transfer === this.transferring?.id) this.transferAcks.add(site);
          this.match[site] = Math.max(this.match[site]!, res.match);
          this.next[site] = res.match + 1;
          if (res.promise && !req.transfer) this.promisedAt[site] = Math.max(this.promisedAt[site] ?? 0, sentAt);
          this.leaseEnd = Math.max(...this.others().map((s) => this.promisedAt[s] ?? 0)) + LEASE;
          this.roundWaiters = this.roundWaiters.filter((w) => (w.since <= sentAt ? (w.resolve(true), false) : true));
          this.advanceCommit();
        } else if (res.refused) {
          this.refusals++;
          this.onRefusal(site, res);
          break;
        } else {
          this.next[site] = Math.max(2, Math.min(from - 1, res.last + 1));
        }
      }
    } finally {
      this.inflight[site] = false;
    }
  }

  /** Pipelined groups: one AppendEntries carrying only entry `index`, sent at once while earlier ones are out. A
   *  follower stores it only if it holds `index - 1` with the same digest, so an out-of-order arrival is refused, not
   *  misapplied, and any refusal or failure hands the follower to `replicate`, which catches it up from `next`. */
  private async sendOne(site: SiteKey, index: number): Promise<void> {
    const term = this.term, e = this.entry(index)!, prev = index - 1;
    const req: AppendReq = { term, leader: this.self, prevIndex: prev, prevTerm: this.logTerm(prev) ?? 0,
      prevDigest: this.entry(prev)?.digest ?? ZERO, entries: [e], commit: this.commitIdx, transfer: this.transferring?.id ?? null };
    const sentAt = Date.now();
    const slot = site === this.others()[0] ? 1 : 3;
    const t = this.timeline.get(index); if (t && !t[slot]) t[slot] = sentAt;
    try {
      const res = await call(() => this.peer(site).append(this.group, req), CALL_DEADLINE, 0);
      this.health(site, true);
      if (res.term > this.term) { this.stepDownTo(res.term); return; }
      if (this.role !== "leader" || this.term !== term) return;
      if (res.steppedDownAt !== undefined) this.peerSteppedDownAt[site] = res.steppedDownAt;
      if (!res.ok) { this.heartbeat[site] = true; void this.replicate(site); return; }
      const t2 = this.timeline.get(index); if (t2 && !t2[slot + 1]) t2[slot + 1] = Date.now();
      (this.appendMs[site] ??= []).push(Date.now() - sentAt);
      (this.serveMs[site] ??= []).push(res.serveMs ?? -1);
      this.match[site] = Math.max(this.match[site] ?? 0, res.match);
      this.next[site] = Math.max(this.next[site] ?? 0, res.match + 1);
      if (res.promise && !req.transfer) this.promisedAt[site] = Math.max(this.promisedAt[site] ?? 0, sentAt);
      this.leaseEnd = Math.max(...this.others().map((s) => this.promisedAt[s] ?? 0)) + LEASE;
      this.roundWaiters = this.roundWaiters.filter((w) => (w.since <= sentAt ? (w.resolve(true), false) : true));
      this.advanceCommit();
    } catch (err) {
      this.health(site, false, err);
      this.heartbeat[site] = true;
      void this.replicate(site);
    }
  }

  /** Commit the highest index of this term that a majority (the leader plus one follower) stores. */
  private advanceCommit(): void {
    const best = Math.max(...this.others().map((s) => this.match[s] ?? 0));
    if (best <= this.commitIdx || this.logTerm(best) !== this.term) return;
    this.commitIdx = best;
    // Anything that reveals this commit (a ticket, a read) must leave after the leader's own copy is written, so the
    // output gate holds it until that write is confirmed. Usually `flush` has already run; this makes it certain.
    if ([...this.unpersisted.keys()].some((i) => i <= best)) this.flush(); else this.scheduleFlush();
    if (this.commitIdx >= this.noopIndex && !this.ready) { this.ready = true; this.event(`ready: noop ${this.noopIndex} committed in term ${this.term}`); }
    // The `Release` group tells its followers each commit at once, so every member's applied state, which isolates read
    // their view from, carries a committed `SiteMode`; otherwise followers apply it only with the group's next entry.
    if (this.group === RELEASE_GROUP) for (const s of this.others()) { this.heartbeat[s] = true; void this.replicate(s); }
    while (this.pending.length && this.commitIdx >= this.pending[0].index) {
      const e = this.pending.shift()!;
      const now = Date.now();
      this.entrySizes.push(e.seqs.length);
      this.entryMs.push(now - e.started);
      const tl = this.timeline.get(e.index); if (tl) tl[5] = now;
      for (const [rid, seq] of this.ridSeq) if (e.seqs.includes(seq)) this.ridSeq.delete(rid);
      for (const seq of e.seqs) {
        const w = this.tickets.get(seq);
        if (w && !w.result) {
          this.commitLat.push(now - w.at);
          w.result = { out: e.outs.get(seq)!, commitMs: now - w.at, entry: e.index }; w.resolve(w.result);
        }
      }
    }
    this.pump();
  }

  /** One AppendEntries round to every follower; true once a majority answers for a round sent at or after `since`. */
  private round(since: number): Promise<boolean> {
    return new Promise((resolve) => {
      const w = { since, resolve };
      this.roundWaiters.push(w);
      setTimeout(() => { this.roundWaiters = this.roundWaiters.filter((x) => x !== w); resolve(false); }, CALL_DEADLINE * (1 + COMMIT_RETRIES));
      for (const s of this.others()) { this.heartbeat[s] = true; void this.replicate(s); }
    });
  }

  /** Group commit: one entry in flight. Each command's decision sees the fold plus earlier commands of its entry. */
  private pump(): void {
    if (this.role !== "leader" || !this.ready || this.pending.length >= this.pipeDepth || this.transferring || !this.queue.length) return;
    if (this.noLeadUntil === this.release) return; // handing off; endLeadership answers the queue NotLeader
    const batch = this.queue.splice(0, BATCH_MAX);
    const overlay = new Map<string, string | null>();
    const events: Ev[] = [], requests: { rid: string; out: Out }[] = [], seqs: number[] = [], outs = new Map<number, Out>();
    for (const c of batch) {
      const prior = this.priorOut(c.rid);
      const w = this.tickets.get(c.seq)!;
      if (prior) { this.ridSeq.delete(c.rid); w.result = { out: prior.out, commitMs: 0, entry: prior.idx }; w.resolve(w.result); continue; }
      const val = (k: string) => (overlay.has(k) ? overlay.get(k)! : this.foldGet(k, this.last));
      const cur = val(c.op.key);
      let out: Out;
      if (c.op.kind === "mode") {
        const d = decideMode(c.op, val);
        if ("refused" in d) { out = { ok: false, value: d.refused }; this.raise(`release group refused ${c.op.by}-key ${c.op.mode} of ${c.op.site}: ${d.refused}`); }
        else { for (const ev of d.events) { events.push(ev); overlay.set(ev.k, ev.v); } out = { ok: true }; }
      } else if (c.op.kind === "put") { events.push({ k: c.op.key, v: c.op.value }); overlay.set(c.op.key, c.op.value); out = { ok: true }; }
      else if (c.op.kind === "cas" && cur === c.op.expect) {
        events.push({ k: c.op.key, v: c.op.value, expect: c.op.expect }); overlay.set(c.op.key, c.op.value); out = { ok: true };
      } else out = { ok: false };
      requests.push({ rid: c.rid, out }); seqs.push(c.seq); outs.set(c.seq, out);
    }
    if (!seqs.length) return this.pump();
    // The new record shape (`WRITES` 2) is written only by the rare command it belongs to: keys starting "new:".
    const body: Body = this.writes > 1 && events.some((ev) => ev.k.startsWith("new:"))
      ? { kind: "events", events, requests, format: this.writes } : { kind: "events", events, requests };
    const e = { term: this.term, index: this.last + 1, prev: this.lastDigest, at: Date.now(), body };
    this.appendLocal({ ...e, digest: digestOf(e) });
    this.timeline.set(e.index, [e.at, 0, 0, 0, 0, 0]);
    if (this.timeline.size > 2000) this.timeline.delete(this.timeline.keys().next().value!);
    this.pending.push({ index: e.index, started: e.at, seqs, outs });
    this.maxPending = Math.max(this.maxPending, this.pending.length);
    for (const s of this.others()) void (this.pipeDepth > 1 ? this.sendOne(s, e.index) : this.replicate(s));
    this.scheduleFlush();
  }

  /** Followers check entry k+1 against their fold at k before storing it (design/DESIGN.md, `verify`). */
  private verify(e: Entry): string | null {
    if (e.prev !== this.lastDigest) return `prev digest mismatch at ${e.index}`;
    if (e.digest !== digestOf({ term: e.term, index: e.index, prev: e.prev, at: e.at, body: e.body })) return `bad digest at ${e.index}`;
    if (e.body.kind !== "events") return null;
    if ((e.body.format ?? 1) > this.reads) return `format ${e.body.format} at ${e.index} is unreadable by release ${this.release}`;
    if (this.breakKind === "refuse") return `injected refusal at ${e.index} by release ${this.release}`;
    const running = new Map<string, string | null>();
    const valueBefore = (k: string): string | null => {
      if (running.has(k)) return running.get(k)!;
      for (let i = this.last; i > this.applied; i--) {
        const b = this.entry(i)!.body;
        if (b.kind === "events") { const ev = [...b.events].reverse().find((x) => x.k === k); if (ev) return ev.v; }
      }
      return this.kvGet(k);
    };
    for (const ev of e.body.events) {
      if (ev.expect !== undefined && valueBefore(ev.k) !== ev.expect) return `cas expect failed for ${ev.k} at ${e.index}`;
      running.set(ev.k, ev.v);
    }
    return null;
  }

  // ---- RPC: member to member ----

  async append(group: string, req: AppendReq): Promise<AppendRes> {
    this.setGroup(group);
    const now = Date.now();
    const no = (extra: Partial<AppendRes> = {}): AppendRes => ({ term: this.term, ok: false, match: 0, last: this.last, promise: false,
      release: this.release, steppedDownAt: this.steppedDownAt, ...extra });
    if (req.term < this.term) return no();
    this.write("append");
    if (req.term > this.term || this.role !== "follower") this.stepDownTo(req.term);
    this.leader = req.leader; this.lastLeaderSeen = now;
    this.transferAccepted = req.transfer;
    const promise = !req.transfer && now >= this.withholdUntil;
    if (promise) { this.promiseUntil = Math.max(this.promiseUntil, now + ELECTION_AFTER); this.promisedTo = req.leader; }
    if (req.prevIndex > this.last) { this.save(); return no({ promise }); }
    // Entries match by digest, not term: a restarted leader that resumes `led_term` may have lost an entry it sent
    // before its own write was confirmed, and then sends a different entry at the same (term, index). Such an entry
    // was never committed, because a commit is revealed only after the leader's own write is confirmed.
    if (req.prevIndex > 0 && this.entry(req.prevIndex)?.digest !== req.prevDigest) {
      this.truncateFrom(req.prevIndex); this.save(); return no({ promise, last: this.last });
    }
    for (const e of req.entries) {
      if (e.index <= this.last) {
        if (this.entry(e.index)?.digest === e.digest) continue;
        this.truncateFrom(e.index);
      }
      try { this.applyUpTo(Math.min(req.commit, e.index - 1)); }
      catch (err) { this.refusedByMe++; this.save(); return no({ promise, refused: String(err).slice(0, 120), refusedAt: e.index, release: this.release }); }
      const refused = this.verify(e);
      if (refused) { this.refusedByMe++; this.save(); return no({ promise, refused, refusedAt: e.index, release: this.release }); }
      this.storeEntry(e);
    }
    this.commitIdx = Math.max(this.commitIdx, Math.min(req.commit, req.prevIndex + req.entries.length));
    try { this.applyUpTo(this.commitIdx); } catch (err) { this.event(`apply failed: ${String(err).slice(0, 80)}`); }
    this.save();
    // The output gate holds the reply until these writes are confirmed anyway; awaiting sync() only makes that wait
    // visible in `serveMs`.
    const t1 = Date.now();
    if (req.entries.length) await this.ctx.storage.sync();
    // F35: a follower's slow confirmations, to compare with its site's leaders' (`storage confirm took`).
    if (Date.now() - t1 > 500) this.event(`follower confirm took ${Date.now() - t1} ms for ${req.entries.length} entries`);
    if (this.slowUntil > Date.now()) await this.gate();
    this.confirmSplit.push([t1 - now, Date.now() - t1]);
    return { term: this.term, ok: true, match: req.prevIndex + req.entries.length, last: this.last, promise, serveMs: Date.now() - now,
      release: this.release, steppedDownAt: this.steppedDownAt };
  }

  async vote(group: string, req: VoteReq, pre: boolean): Promise<VoteRes> {
    this.setGroup(group);
    // A promise that lapses within this call's deadline: answer once it lapses rather than refuse and make the
    // candidate back off. Every check below runs after the wait, on current state.
    const wait = this.promiseUntil - Date.now();
    if (wait > 0 && wait <= CALL_DEADLINE / 2 && this.role !== "leader") await sleep(wait + 1);
    const now = Date.now();
    const deny = (why: string): VoteRes => { this.event(`${pre ? "pre-" : ""}vote denied to ${req.candidate} for ${req.term}: ${why}`); return { term: this.term, granted: false, why }; };
    if (req.term < this.term) return deny("stale term");
    const transferVote = req.transfer !== null &&
      (req.transfer === this.transferAccepted || (req.transfer === this.transferring?.id && req.candidate === this.transferring.to));
    if (!transferVote) {
      if (this.role === "leader") return deny("leader");
      if (now < this.promiseUntil && req.candidate !== this.promisedTo) return deny("promised");
    }
    if (!pre && req.term > this.term) this.stepDownTo(req.term);
    if (req.lastTerm < this.lastTerm || (req.lastTerm === this.lastTerm && req.lastIndex < this.last)) return deny("log behind");
    // Two members campaigning at once: the lower site key wins the pre-vote, so the higher one stands down. Only when
    // this member could win: a candidate whose log is newer than ours beats us anyway, and a member that the others
    // refuse as suspected (`suspectedUntil`) cannot win.
    const newer = req.lastTerm > this.lastTerm || (req.lastTerm === this.lastTerm && req.lastIndex > this.last);
    if (pre && !transferVote && this.campaigning && req.candidate > this.self && !newer && now >= this.suspectedUntil) return deny("contending");
    if (pre && !transferVote) {
      const flapping = req.boots[0] > now - RESET_QUARANTINE && req.boots[1] > now - RESET_QUARANTINE;
      const steppedDown = req.steppedDownAt !== null && req.steppedDownAt > now - RESET_QUARANTINE;
      const suspected = (this.suspected.get(req.candidate) ?? 0) > now;
      if ((flapping || steppedDown || suspected) && now - this.lastLeaderSeen < 2 * ELECTION_AFTER) return deny(suspected ? "suspected" : "quarantine");
      // A candidate that cannot lead loses to a voter that can and could win, whose log is at least as current, and
      // that voter campaigns instead. A voter whose log is behind could never win, so it does not stand in the way.
      if (req.cannotLead && !newer && (await this.leadBar()) === null && this.role === "follower") {
        if (!this.campaigning) void this.campaign(null);
        return deny("voter can lead");
      }
      return { term: this.term, granted: true };
    }
    if (pre) return { term: this.term, granted: true };
    if (this.votedFor && this.votedFor !== req.candidate) return deny("voted");
    this.write("vote");
    this.votedFor = req.candidate;
    this.save();
    return { term: this.term, granted: true };
  }

  async timeoutNow(group: string, transfer: string): Promise<Verdict> {
    this.setGroup(group);
    return this.campaign(transfer);
  }

  // ---- RPC: edge to member ----

  /** Edge to a follower after a call to the leader misses CALL_DEADLINE (design/DESIGN.md, `suspect`). */
  /** `target` is the member the caller failed to reach. A report counts against this member's leader only if it names
   *  that leader (design/DESIGN.md: `suspect` carries the term), so late reports about a deposed leader never withhold
   *  promises from its successor. */
  async suspect(group: string, colo: string, target?: SiteKey): Promise<Verdict> {
    this.setGroup(group);
    if (this.role === "leader") return "leader";
    const now = Date.now();
    if (this.promiseUntil - now <= CALL_DEADLINE) this.event(`suspect from ${colo}, promise ${this.promiseUntil - now} ms left`);
    if (now < this.promiseUntil) {
      if (target !== undefined && target !== this.leader) return "promised";
      this.reports.set(colo, now);
      for (const [c, t] of this.reports) if (t < now - SUSPECT_WINDOW) this.reports.delete(c);
      if (this.reports.size >= SUSPECT_COLOS) {
        this.withholdUntil = now + 2 * SUSPECT_WINDOW;
        if (this.leader && this.leader !== this.self) this.suspected.set(this.leader, now + 2 * SUSPECT_WINDOW);
      }
      // A promise that lapses within this call's deadline: hold the report until it lapses and campaign then, still
      // inside the request, so failover does not wait for the edge's next retry.
      const left = this.promiseUntil - now;
      if (left > CALL_DEADLINE / 2) return "promised";
      await sleep(left + 1);
      const role = this.role as string; // a campaign may have finished during the sleep
      if (Date.now() < this.promiseUntil || role === "leader") return role === "leader" ? "leader" : "promised";
    }
    return this.campaign(null);
  }

  /** Leads this call if it can: a leader, a restarted leader resuming `led_term`, or the member that takes a new group's
   *  first call. */
  private async canLead(): Promise<boolean> {
    if (this.role === "leader") return true;
    if (this.ledTerm === this.term) return this.resume();
    if (this.term === 1 && this.last === 1) return (await this.campaign(null)) === "leader";
    return false;
  }

  async commit(group: string, rid: string, op: RaftOp): Promise<{ admitted: Ticket } | { done: Out } | { notLeader: { term: number; leader: SiteKey | null } }> {
    this.setGroup(group);
    this.count("commit");
    if (!(await this.canLead())) return { notLeader: { term: this.term, leader: this.leader } };
    this.lastServed = Date.now(); this.armDrainCheck();
    void this.maybeHandOff();
    const t0 = Date.now();
    const prior = this.priorOut(rid);
    if (Date.now() - t0 > 50 && this.slowNotes++ < 20) this.event(`commit: priorOut took ${Date.now() - t0} ms`);
    if (prior) return { done: prior.out };
    // A loop that gave up on an unreachable follower restarts on the next command.
    if (this.pending.length) for (const s of this.others()) void this.replicate(s);
    const joined = this.ridSeq.get(rid);
    if (joined !== undefined) return { admitted: { term: this.term, boot: this.boot, seq: joined } };
    const seq = ++this.seq;
    let resolve!: (v: OutcomeRes) => void;
    const promise = new Promise<OutcomeRes>((r) => (resolve = r));
    this.tickets.set(seq, { resolve, promise, at: Date.now() });
    for (const [s, w] of this.tickets) if (w.result && Date.now() - w.at > 60_000) this.tickets.delete(s);
    this.ridSeq.set(rid, seq);
    this.queue.push({ rid, op, seq });
    this.pump();
    return { admitted: { term: this.term, boot: this.boot, seq } };
  }

  async outcome(group: string, ticket: Ticket): Promise<OutcomeRes> {
    this.setGroup(group);
    this.count("outcome");
    const w = this.tickets.get(ticket.seq);
    if (ticket.term !== this.term || ticket.boot !== this.boot || !w) return { notLeader: { term: this.term, leader: this.role === "leader" ? this.self : this.leader } };
    const res = await Promise.race([w.promise, sleep(15_000).then((): OutcomeRes => ({ notLeader: { term: this.term, leader: null } }))]);
    this.tickets.delete(ticket.seq);
    return res;
  }

  /** `fresh`: only a ready leader answers, from its fold under the lease, else after one read-index round. Every read
   *  touches SQLite, so a replaced instance cannot answer. */
  async read(group: string, key: string): Promise<ReadRes> {
    this.setGroup(group);
    this.count("read");
    const arrived = Date.now();
    if (!(await this.canLead())) return { notLeader: { term: this.term, leader: this.leader } };
    this.lastServed = Date.now(); this.armDrainCheck();
    void this.maybeHandOff();
    if (this.transferring) return { busy: "transfer" };
    for (let i = 0; !this.ready && i < 20 && this.role === "leader"; i++) await sleep(50);
    if (!this.ready) return { busy: "noop not committed" };
    let viaRound = false;
    if (Date.now() >= this.leaseEnd - DRIFT) {
      if (!(await this.round(arrived))) return { notLeader: { term: this.term, leader: null } };
      viaRound = true;
    }
    if (this.role !== "leader") return { notLeader: { term: this.term, leader: this.leader } };
    return { out: { value: this.foldGet(key, this.commitIdx) }, viaRound };
  }

  // ---- RPC: learners ----

  /** Leader side of read-index: once this leader's term `Noop` is committed and its lease holds (or after one round),
   *  the commit index, plus committed entries from `from` so the learner can catch up in the same call. */
  async readIndex(group: string, from: number): Promise<IndexRes> {
    this.setGroup(group);
    this.count("readIndex");
    const arrived = Date.now();
    if (this.role !== "leader" || this.transferring) return { notLeader: { term: this.term, leader: this.role === "leader" ? this.self : this.leader } };
    for (let i = 0; !this.ready && i < 20 && this.role === "leader"; i++) await sleep(50);
    if (!this.ready) return { notLeader: { term: this.term, leader: null } };
    if (Date.now() >= this.leaseEnd - DRIFT && !(await this.round(arrived))) return { notLeader: { term: this.term, leader: null } };
    if (this.role !== "leader") return { notLeader: { term: this.term, leader: this.leader } };
    const index = this.commitIdx, entries: Entry[] = [];
    for (let i = Math.max(2, from); i <= Math.min(index, from + CATCHUP_MAX - 1); i++) entries.push(this.entry(i)!);
    return { index, entries };
  }

  /** A learner's `fresh` read (design/DESIGN.md, `MemberKind::Learner`): it never votes and never leads, and it answers
   *  only after a read-index request sent after the read arrived, once it has applied that index. One request is in
   *  flight at a time and at most one starts every LEARNER_GAP, so a learner adds at most 20 leader calls a second.
   *  In this prototype a learner pulls committed entries in that same call, instead of the leader pushing them. */
  async learnerRead(group: string, key: string): Promise<ReadRes> {
    this.setGroup(group);
    this.count("learnerRead");
    const arrived = Date.now();
    const index = await new Promise<number | null>((resolve) => { this.riWaiters.push({ since: arrived, resolve }); this.kick(); });
    if (index === null) return { busy: "no leader answered read-index" };
    this.learnerLat.push(Date.now() - arrived);
    return { out: { value: this.foldGet(key, index) }, viaRound: false };
  }
  /** B0-18 load: `rate` reads a second for `seconds`, issued inside this learner through `learnerRead`'s own path, so the
   *  read-index batching and the leader's load are real, but the platform's per-object request rate is not exercised. */
  async selfLoad(group: string, seconds: number, rate: number, keys: number): Promise<{ n: number; failed: number; lat: number[] }> {
    this.setGroup(group);
    const end = Date.now() + seconds * 1000, lat: number[] = [];
    let n = 0, failed = 0;
    const pending: Promise<void>[] = [];
    while (Date.now() < end) {
      const t = Date.now();
      for (let i = 0; i < rate / 100; i++) {
        n++;
        pending.push(this.learnerRead(group, `k${Math.floor(Math.random() * keys)}`).then((r) => {
          if ("out" in r) lat.push(Date.now() - t); else failed++;
        }));
      }
      await sleep(Math.max(0, 10 - (Date.now() - t)));
    }
    await Promise.all(pending);
    return { n, failed, lat };
  }
  private kick(): void {
    if (this.riBusy || !this.riWaiters.length) return;
    const wait = this.riLast + LEARNER_GAP - Date.now();
    if (wait > 0) { this.riBusy = true; setTimeout(() => { this.riBusy = false; this.kick(); }, wait); return; }
    this.riBusy = true;
    const sentAt = this.riLast = Date.now();
    void this.pullIndex().then((index) => {
      // Only reads that arrived before this request was sent may use its index.
      const ready = this.riWaiters.filter((w) => w.since <= sentAt);
      this.riWaiters = this.riWaiters.filter((w) => w.since > sentAt);
      for (const w of ready) w.resolve(index);
    }).finally(() => { this.riBusy = false; this.kick(); });
  }
  /** One read-index request, following a named leader, then catch-up pulls until this learner has applied the index. */
  private async pullIndex(): Promise<number | null> {
    for (let attempt = 0; attempt < 4; attempt++) {
      const leader = this.learnerOf;
      let r: IndexRes;
      try { r = await call(async () => (await this.peer(leader).readIndex(this.group, this.last + 1)) as IndexRes, CALL_DEADLINE * 3, 0); }
      catch { this.learnerOf = SITES[(SITES.indexOf(leader) + 1) % 3]; continue; }
      if ("notLeader" in r) { this.learnerOf = r.notLeader.leader && r.notLeader.leader !== leader ? r.notLeader.leader : SITES[(SITES.indexOf(leader) + 1) % 3]; continue; }
      const index = r.index;
      this.absorb(r.entries);
      while (this.last < index) {
        const more = await call(async () => (await this.peer(leader).readIndex(this.group, this.last + 1)) as IndexRes, CALL_DEADLINE * 3, 0).catch(() => null);
        if (!more || "notLeader" in more || !more.entries.length) return null;
        this.absorb(more.entries);
      }
      this.commitIdx = Math.max(this.commitIdx, index);
      this.applyUpTo(this.commitIdx);
      this.save();
      return index;
    }
    return null;
  }
  /** Stores committed entries that extend this learner's log; they chain by digest, as `verify` checks. */
  private absorb(entries: Entry[]): void {
    for (const e of entries) {
      if (e.index !== this.last + 1 || e.prev !== this.lastDigest) break;
      this.storeEntry(e);
    }
  }

  // ---- RPC: test control ----

  /** Raft leadership transfer. `abandon` stops after catching `to` up, as an abandoned transfer does. */
  async transfer(group: string, to: SiteKey, abandon: boolean): Promise<string> {
    this.setGroup(group);
    if (this.role !== "leader" || to === this.self) return "not leader";
    const id = crypto.randomUUID();
    this.transferring = { to, id }; this.transferAcks.clear();
    const deadline = Date.now() + LEASE;
    while (this.pending.length && Date.now() < deadline) await sleep(10);
    for (const s of this.others()) { this.heartbeat[s] = true; void this.replicate(s); }
    while ((this.match[to] ?? 0) < this.last && Date.now() < deadline && this.role === "leader") await sleep(10);
    // The target needs one vote besides its own. This leader grants it, but on a gray leader's site that vote call is
    // the one most likely lost, and the other follower grants only after it has seen this transfer's id; otherwise it
    // refuses ("promised"), the target's failed campaign still deposes this leader, and an idle group stays without a
    // leader until its next request (B0-07). So wait up to 2 x CALL_DEADLINE for the other follower's ack too.
    const other = this.others().find((s) => s !== to)!, primed = Date.now();
    while (!this.transferAcks.has(other) && Date.now() - primed < 2 * CALL_DEADLINE && Date.now() < deadline && this.role === "leader") await sleep(10);
    let verdict: Verdict | "abandoned" = "abandoned";
    if (!abandon && this.role === "leader" && (this.match[to] ?? 0) >= this.last) {
      // Retried like any commit-path call, and then until the transfer's LEASE runs out: on a gray link three lost
      // `TimeoutNow` calls in a row abandoned drain hand-offs (B0-07). A repeat is harmless: the target
      // answers "leader" once it has won with this transfer id, and once its vote request reaches this leader, this
      // leader steps down and the loop ends.
      for (let i = 0; verdict !== "leader" && this.role === "leader" && (i <= COMMIT_RETRIES || Date.now() < deadline); i++) {
        verdict = await call(() => this.peer(to).timeoutNow(this.group, id), CALL_DEADLINE, 0).catch(() => "lost" as Verdict);
        if (verdict !== "leader") await sleep(100);
      }
    }
    if (verdict === "leader") { this.leader = to; this.ledTerm = null; this.stepDownTo(this.term); return "transferred"; }
    if (this.role !== "leader") return `transferred, reply lost (${verdict})`;
    // Abandoned: send transfer: None and serve `fresh` again only after one full round.
    this.transferring = null; this.leaseEnd = 0;
    await this.round(Date.now());
    this.pump();
    return `abandoned (${verdict})`;
  }

  async reset(): Promise<void> { this.ctx.abort("m0 reset"); }

  /** Read-only: stored log entries whose `at` falls in [from, to], with this member's commit and applied indexes. */
  async dump(group: string, from: number, to: number): Promise<{ commit: number; applied: number; last: number; entries: Entry[] }> {
    this.setGroup(group);
    const rows = this.sql.exec<{ idx: number; term: number; prev: string; digest: string; at: number; body: string }>(
      `SELECT * FROM log WHERE at BETWEEN ? AND ? ORDER BY idx`, from, to).toArray();
    return { commit: this.commitIdx, applied: this.applied, last: this.last,
      entries: rows.map((r) => ({ index: r.idx, term: r.term, prev: r.prev, digest: r.digest, at: r.at, body: JSON.parse(r.body) })) };
  }

  ping(): number { return Date.now(); }
  /** B0-04 probe: a call that stays open for `ms`, standing in for a follower that restarts or hangs. */
  async hold(ms: number): Promise<number> { await sleep(ms); return Date.now(); }
  /** B0-04 probe: starts `n` calls to `slow`'s `hold(ms)` and lets each miss CALL_DEADLINE as `call` does, then times
   *  pings to `fast` from this same invocation. Does a call abandoned at its deadline still hold an open-connection
   *  slot, so that calls to a healthy follower queue behind it? */
  async connProbe(group: string, slow: SiteKey, fast: SiteKey, n: number, ms: number): Promise<{ missed: number; pingMs: number[] }> {
    this.setGroup(group);
    const r = await Promise.allSettled(Array.from({ length: n }, () => call(() => this.peer(slow).hold(ms), CALL_DEADLINE, 0)));
    const pingMs: number[] = [];
    for (let i = 0; i < 3; i++) { const t = Date.now(); await call(() => this.peer(fast).ping(), 10_000, 0).catch(() => 0); pingMs.push(Date.now() - t); }
    return { missed: r.filter((x) => x.status === "rejected").length, pingMs };
  }
  echo(s: string): number { return s.length; }

  /** F35: this object's storage alone. Every `every` ms for `ms`, one small insert and `sync()`, timed, with no Raft
   *  work in the object. Returns [start, confirm ms] per write. */
  async syncProbe(ms: number, every: number, bytes = 1_000, alarmEvery = 0): Promise<{ colo: string; samples: [number, number][] }> {
    await this.status();
    let alarmAt = 0;
    this.sql.exec(`CREATE TABLE IF NOT EXISTS probe (i INTEGER PRIMARY KEY, b TEXT)`);
    const samples: [number, number][] = [], end = Date.now() + ms;
    while (Date.now() < end) {
      const t = Date.now();
      if (alarmEvery && t - alarmAt >= alarmEvery) { alarmAt = t; void this.ctx.storage.setAlarm(t + 60_000, { allowUnconfirmed: true }); }
      this.sql.exec(`INSERT INTO probe (b) VALUES (?)`, "x".repeat(bytes));
      await this.ctx.storage.sync();
      samples.push([t, Date.now() - t]);
      await sleep(Math.max(0, every - (Date.now() - t)));
    }
    this.sql.exec(`DELETE FROM probe`);
    if (alarmEvery) await this.ctx.storage.deleteAlarm();
    return { colo: this.colo, samples };
  }

  /** Splits a replication round: a plain binding round trip to each follower, and this member's own write
   *  confirmation (`sync()` after a log-sized insert), each sampled `n` times. */
  async probe(group: string, n: number): Promise<{ colo: string; pingMs: Record<string, number[]>; confirmMs: number[] }> {
    this.setGroup(group);
    await this.status();
    // The tail of a plain round trip to the nearest follower, idle and sequential.
    const tail: number[] = [];
    for (let i = 0; i < (n > 50 ? n : 0); i++) { const t = Date.now(); await this.peer(this.others()[0]).ping(); tail.push(Date.now() - t); }
    if (n > 50) return { colo: this.colo, tail } as unknown as { colo: string; pingMs: Record<string, number[]>; confirmMs: number[] };
    const pingMs: Record<string, number[]> = {};
    for (const s of this.others()) {
      pingMs[s] = [];
      for (let i = 0; i < n; i++) { const t = Date.now(); await this.peer(s).ping(); pingMs[s].push(Date.now() - t); }
    }
    this.sql.exec(`CREATE TABLE IF NOT EXISTS probe (i INTEGER PRIMARY KEY, b TEXT)`);
    const confirmMs: number[] = [];
    for (let i = 0; i < n; i++) {
      this.sql.exec(`INSERT INTO probe (b) VALUES (?)`, "x".repeat(16_000));
      const t = Date.now(); await this.ctx.storage.sync(); confirmMs.push(Date.now() - t);
    }
    // Does a write gate a call to a follower? Ping after a write, and a ping started before the same write.
    const gatedMs: number[] = [], overlapMs: number[] = [];
    const f = this.others()[0];
    for (let i = 0; i < n; i++) {
      this.sql.exec(`INSERT INTO probe (b) VALUES (?)`, "x".repeat(16_000));
      let t = Date.now(); await this.peer(f).ping(); gatedMs.push(Date.now() - t);
      t = Date.now(); const p = this.peer(f).ping();
      this.sql.exec(`INSERT INTO probe (b) VALUES (?)`, "x".repeat(16_000));
      await p; overlapMs.push(Date.now() - t);
    }
    // A call started, then a macrotask yield, then the write: is the call already on the wire?
    const yieldMs: number[] = [], plainMs: number[] = [];
    for (let i = 0; i < n; i++) {
      let t = Date.now(); const p = this.peer(f).ping();
      await new Promise((r) => setTimeout(r, 0));
      this.sql.exec(`INSERT INTO probe (b) VALUES (?)`, "x".repeat(16_000));
      await p; yieldMs.push(Date.now() - t);
      await this.ctx.storage.sync();
      t = Date.now(); await this.peer(f).ping(); plainMs.push(Date.now() - t);
    }
    // Payload size: a ping carrying 20 KB and 50 KB, as an AppendEntries does.
    const big20: number[] = [], big50: number[] = [], big500: number[] = [], big1500: number[] = [];
    for (let i = 0; i < n; i++) {
      let t = Date.now(); await this.peer(f).echo("x".repeat(20_000)); big20.push(Date.now() - t);
      t = Date.now(); await this.peer(f).echo("x".repeat(50_000)); big50.push(Date.now() - t);
      t = Date.now(); await this.peer(f).echo("x".repeat(500_000)); big500.push(Date.now() - t);
      t = Date.now(); await this.peer(f).echo("x".repeat(1_500_000)); big1500.push(Date.now() - t);
    }
    this.sql.exec(`DELETE FROM probe`);
    return { colo: this.colo, pingMs, confirmMs, gatedMs, overlapMs, yieldMs, plainMs, big20, big50, big500, big1500 } as { colo: string; pingMs: Record<string, number[]>; confirmMs: number[] };
  }

  /** Leader-side commit latency, admission to quorum ack, since the last reset of these counters. */
  async stats(reset: boolean): Promise<{ site: SiteKey; role: string; n: number; entries: number; lat: number[]; entryMs: number[]; appendMs: Partial<Record<SiteKey, number[]>>;
    serveMs: Partial<Record<SiteKey, number[]>>; bytesOut: number[]; confirmSplit: [number, number][]; timeline: number[][]; log: string[];
    reqs: Record<string, number>; reqsSeconds: number; learnerLat: number[]; maxPending: number; faults: ReturnType<typeof isolateFaultStats> }> {
    const out = { site: this.self, role: this.role, n: this.commitLat.length, entries: this.entrySizes.length, lat: [...this.commitLat],
      entryMs: [...this.entryMs], appendMs: { ...this.appendMs }, serveMs: { ...this.serveMs }, bytesOut: [...this.bytesOut], confirmSplit: this.confirmSplit.splice(0),
      timeline: [...this.timeline.entries()].map(([i, t]) => [i, ...t]), log: [...this.log],
      reqs: { ...this.reqs }, reqsSeconds: (Date.now() - this.reqsSince) / 1000, learnerLat: [...this.learnerLat], maxPending: this.maxPending,
      faults: isolateFaultStats() };
    if (reset) { this.maxPending = 0; this.timeline.clear(); this.log = []; this.reqs = {}; this.reqsSince = Date.now(); this.learnerLat = []; }
    if (reset) { this.commitLat = []; this.entrySizes = []; this.entryMs = []; this.appendMs = {}; this.serveMs = {}; this.bytesOut = []; }
    return out;
  }

  private colo = "";
  async status(): Promise<MemberStatus> {
    if (!this.colo) {
      const t = await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text();
      this.colo = /^colo=(\w+)$/m.exec(t)?.[1] ?? "unknown";
    }
    return { colo: this.colo, site: this.self, role: this.role, term: this.term, votedFor: this.votedFor, ledTerm: this.ledTerm, leader: this.leader,
      last: this.last, commit: this.commitIdx, applied: this.applied, promiseUntil: this.promiseUntil, leaseEnd: this.leaseEnd,
      boots: [this.boots[0], this.boots[1]], refusals: this.refusals, steppedDownAt: this.steppedDownAt,
      release: this.release, noLeadUntil: this.noLeadUntil, cannotFollow: this.cannotFollow, applyErrors: this.applyErrors,
      refusedByMe: this.refusedByMe, alarms: [...this.alarms], stepDowns: this.stepDowns, modes: modesView?.modes };
  }
}

export default {
  async fetch(): Promise<Response> { return new Response("not found", { status: 404 }); },
} satisfies ExportedHandler<MemberEnv>;
