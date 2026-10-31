// M0-WP9 copy of the M0-WP2 Raft member (src/raft/member.ts, copied 2026-10-29 and not kept in sync), deployed as
// graft-wp9-a (enam), graft-wp9-b (weur), and graft-wp9-c (apac). What this copy adds (build/BUILD-PLAN.md, M0-WP9;
// design/DESIGN.md, Shape, "Agent load and agent safety"; design/AGENT-LAYER.md, Shape, "Activity feed"):
//
// - Leader admission inside group commit: every queued command has an `AdmitClass`; each entry takes Human and
//   MergeQueue first, then Agent up to AGENT_FLOOR of capacity, then Bot, then Agent up to AGENT_SHARE, with fair
//   queuing by actor inside Agent. Capacity is measured entries/s times commands per entry at the offered byte mix,
//   unless the group is named "capN-..." (a fixed N commands/s). An actor past DEMOTE_PER_MIN writes a minute on the
//   group is classed Agent. Agent overflow waits up to AGENT_WAIT and then gets the refusal text; AGENT_QUEUE_MAX caps
//   queued agent commands and bytes, and under pressure an inline agent push is answered "stage" so the edge stores
//   its pack first (`PackRef::Stored`).
// - Hot groups: above HOT_PUSHES pushes/s the commit reply tells the edge to inline only up to INLINE_MAX_HOT.
// - Activity feed polls (`feed`), answered only by followers and learners from their applied log, never the leader.
// - Learners fed by the leader (AppendEntries of committed entries, never counted toward a quorum), so they follow the
//   log without read-index calls.
// - Per-class counters for the M0-WP9 rows.
//
// Not changed: the Raft core, terms, leases, `suspect`, `STEP_DOWN`, transfer. Faults and `SiteHealth` are unbound.

import { DurableObject } from "cloudflare:workers";
import { Faults, faultyStub, type FaultControl } from "../faults.ts";
import { sha256hex } from "./sha256.ts";
import {
  CALL_DEADLINE, COMMIT_RETRIES, DRIFT, ELECTION_AFTER, hintFor, LEARNER_GAP, LEASE, MODE_KEYS, mayLead, modesFrom, NO_MODES,
  POINTER_TTL, RELEASE_GROUP, RESET_QUARANTINE, SITES, STEP_DOWN, SUSPECT_COLOS, SUSPECT_WINDOW, memberName, learnerName,
  AGENT_FLOOR, AGENT_QUEUE_MAX, AGENT_SHARE, AGENT_WAIT, DEMOTE_PER_MIN, ENTRY_MAX, FEED_BATCH, FEED_BATCH_ALL, FEED_RING,
  AGENT_INLINE_ENTRY, COMMIT_HOLD, FEED_HELD_MAX, FEED_WAIT_MAX, HOT_PUSHES, INLINE_MAX_HOT, refusalText, variant, type AdmitClass, type Hints, type Meta,
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
export type CommitRes = { admitted: Ticket; hints: Hints } | { done: Out } | { notLeader: { term: number; leader: SiteKey | null } }
  | { refused: string; hints: Hints } | { stage: true; hints: Hints }
  /** Sixth pass: the commit call held until its entry committed, answered with the outcome. */
  | { result: Exclude<OutcomeRes, { notLeader: unknown }>; hints: Hints };
/** `cls` is the class the command was admitted under; `waited` is how many entries after the one in flight at its
 *  admission it landed in (1: the next entry). `refused` marks an AGENT_WAIT refusal, which commits nothing. */
export type OutcomeRes = { out: Out; commitMs: number; entry: number; cls?: AdmitClass; waited?: number; refused?: boolean }
  | { notLeader: { term: number; leader: SiteKey | null } };
export type FeedRes = { cursor: number; events: number; site: SiteKey; heldMs: number } | { notServing: string } | { full: true };
export type IndexRes = { index: number; entries: Entry[] } | { notLeader: { term: number; leader: SiteKey | null } };
export type ReadRes = { out: Out; viaRound: boolean } | { notLeader: { term: number; leader: SiteKey | null } } | { busy: string };

const ZERO = "0".repeat(64);
const BATCH_MAX = 2_000; // commands per entry; 1 KB commands keep an entry under the design's 1.5 MB
const CATCHUP_MAX = 200; // entries per AppendEntries
/** The chained digest covers the entry's inline pack bytes too. */
const digestOf = (e: Omit<Entry, "digest">) => sha256hex(`${e.prev}|${e.term}|${e.index}|${e.at}|${JSON.stringify(e.body)}`, e.packs);
const packBytes = (e: Entry) => e.packs?.byteLength ?? 0;
/** Bytes of an entry as AppendEntries carries it: the JSON body plus the inline packs. */
const entryBytes = (e: Entry) => JSON.stringify(e.body).length + packBytes(e) + 200;
/** SQLite binds an ArrayBuffer, not a view. */
const blob = (p: Uint8Array | null | undefined) => (!p ? null : p.byteOffset === 0 && p.byteLength === p.buffer.byteLength ? p.buffer : p.slice().buffer);
const BOOT_BODY: Body = { kind: "config", voters: SITES };
const BOOT: Entry = { term: 1, index: 1, prev: ZERO, at: 0, body: BOOT_BODY, digest: digestOf({ term: 1, index: 1, prev: ZERO, at: 0, body: BOOT_BODY }) };
/** A boot more than this long after the member last saved its state is an idle eviction, not a reset. Milestone 0
 *  measured idle members evicted after about 10 s, so this sits below that. */
const BOOT_IDLE = 5_000;
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
/** The `Release` view this isolate last read: refreshed from the own site's `release` member at most every
 *  POINTER_TTL, never moved backward, and kept when a refresh fails (design/DESIGN.md, FX-8). */
let modesView: { at: number; applied: number; modes: Modes } | null = null;
let modesRefresh: Promise<void> | null = null;

type Waiter = { resolve: (v: OutcomeRes) => void; promise: Promise<OutcomeRes>; result?: OutcomeRes; at: number; cls: AdmitClass; admitLast: number };
type Queued = { rid: string; op: RaftOp; seq: number; meta: Meta; cls: AdmitClass; bytes: number; at: number; inl: number };
type FeedWaiter = { paths: string[] | null; matched: boolean; resolve: () => void; batch: number };
const CLASSES: AdmitClass[] = ["Human", "MergeQueue", "Bot", "Agent"];
/** Bytes of entries one AppendEntries carries when catching up, so a lagging follower never gets a 300 MB call. */
const CATCHUP_BYTES = 4_000_000;
/** The prototype's `pressure`: see `hints`. */
const PRESSURE_AGE = 1_000;

/** M0-WP9 counters, kept on every member and read by `w9stats`. Times are epoch ms. */
class Member9Stats {
  /** Admission to quorum ack, per admitted class. */
  lat: Record<AdmitClass, number[]> = { Human: [], MergeQueue: [], Bot: [], Agent: [] };
  /** How many entries after the last one at admission each command landed in (1: the next entry), per class. */
  waited: Record<AdmitClass, Record<number, number>> = { Human: {}, MergeQueue: {}, Bot: {}, Agent: {} };
  /** Refusals by cause ("wait": AGENT_WAIT, "queue": AGENT_QUEUE_MAX) and by text with digits replaced by n. */
  refusals: Record<string, number> = {};
  shapes: Record<string, number> = {};
  stale: Record<string, number> = {};
  /** Per second on the leader: [at, capacity, tokens, agent queued, agent queued bytes, other queued, round EWMA ms,
   *  requests so far, outcome requests so far, feed requests so far]. */
  seconds: number[][] = [];
  /** Per entry on the leader: [at, commands, bytes, inline pack bytes, Human, MergeQueue, Bot, Agent, Agent inline pack bytes]. */
  entries: number[][] = [];
  heldPeak = 0; holdExpired = 0;
  demoted = 0; staged = 0; budgetStaged = 0; queueRefused = 0; suspects = 0; leaderStarts = 0;
  feedOnLeader = 0; feedServed = 0; feedRefused = 0; feedFull = 0; feedHeld = 0; feedHeldPeak = 0; feedEvents = 0;
  agentQueuedPeak = 0; agentBytesPeak = 0; learnerAppends = 0;
  /** The leader's own storage confirmations (`STEP_DOWN`'s second input), and the member's leadership events. */
  confirmMs: number[] = []; events: string[] = [];
  since = Date.now();
  static empty(): Member9Stats { return new Member9Stats(); }
  committed(cls: AdmitClass, ms: number, waited: number): void {
    if (this.lat[cls].length < 400_000) this.lat[cls].push(ms);
    this.waited[cls][waited] = (this.waited[cls][waited] ?? 0) + 1;
  }
  refusal(why: string, text: string): void {
    this.refusals[why] = (this.refusals[why] ?? 0) + 1;
    const shape = text.replace(/\d+/g, "n").replace(/^ng \S+/, "ng <ref>");
    this.shapes[shape] = (this.shapes[shape] ?? 0) + 1;
  }
  staleBy(cls: AdmitClass, kind: string): void { const k = `${cls}:${kind}`; this.stale[k] = (this.stale[k] ?? 0) + 1; }
  second(row: number[]): void { if (this.seconds.length < 7_200) this.seconds.push(row); }
  entry(row: number[]): void { if (this.entries.length < 40_000) this.entries.push(row); }
}

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
  private callHealth: [number, SiteKey, boolean][] = [];
  /** Fourth pass, from src/raft/member.ts: followers that have answered an AppendEntries in this leadership; failed
   *  calls to any other are not counted. */
  private heard = new Set<SiteKey>();
  /** Fourth pass: each follower's `stepped_down_at`, from its AppendEntries replies. */
  private peerSteppedDownAt: Partial<Record<SiteKey, number | null>> = {};
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
  private leaseEnd = 0;
  private roundWaiters: { since: number; resolve: (ok: boolean) => void }[] = [];
  // M0-WP9 admission: one FIFO lane per class, and per-actor lanes inside Agent served round-robin.
  private lanes: Record<"Human" | "MergeQueue" | "Bot", Queued[]> = { Human: [], MergeQueue: [], Bot: [] };
  private agentQ = new Map<string, Queued[]>();
  private agentOrder: string[] = [];
  private agentQueued = 0;
  private agentQueuedBytes = 0;
  /** Fifth pass: Agent-class inline pack bytes queued and not yet in an entry, held under AGENT_INLINE_ENTRY on a hot group. */
  private agentInlineQueued = 0;
  private tokens = 0;
  private tokensAt = 0;
  private pumpTimer = false;
  /** Entry round (created to committed) and offered bytes per command, as EWMAs: the inputs to capacity. */
  private roundEwma = 250;
  private cmdBytesEwma = 1_000;
  private actorWrites = new Map<string, number[]>();
  private pushTimes: number[] = [];
  private w9 = Member9Stats.empty();
  /** Parsed bodies of recent stored entries, so `decide` and `verify` do not reparse a large entry per command. */
  private bodyCache = new Map<number, Body>();
  // Activity feed.
  private ring: { index: number; paths: string[]; n: number }[] = [];
  private feedWaiters = new Set<FeedWaiter>();
  private isLearner = false;
  private lastFedAt = 0;
  // Leader-fed learners (slots), one AppendEntries of committed entries in flight per learner.
  private learnerSlots: number[] = [];
  private lNext = new Map<number, number>();
  private lInflight = new Set<number>();
  /** Entries sent and not yet committed, oldest first. At most one, or `pipeDepth` for a pipelined group. */
  private pending: { index: number; started: number; seqs: number[]; outs: Map<number, Out>; bytes: number }[] = [];
  /** B0-01 experiment, outside the design: a group named "plN-..." keeps up to N entries in flight, each sent to each
   *  follower in its own AppendEntries call. Every other group keeps one entry in flight, as the design states. */
  private get pipeDepth(): number { return Number(/^pl(\d+)-/.exec(this.group)?.[1] ?? 1); }
  private tickets = new Map<number, Waiter>();
  private ridSeq = new Map<string, number>();
  private seq = 0;
  /** Random per instance, so tickets from before a reset never match this instance's (design/DESIGN.md, `Ticket`). */
  private readonly boot = crypto.getRandomValues(new Uint32Array(1))[0] * 2 ** 21 + (crypto.getRandomValues(new Uint32Array(1))[0] >>> 11);
  private transferring: { to: SiteKey; id: string } | null = null;
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
  private event(m: string): void {
    this.log.push(`${Date.now()} ${m}`); if (this.log.length > 300) this.log.splice(0, 100);
    if (!m.startsWith("ack ") && !m.startsWith("suspect from") && this.w9 && this.w9.events.length < 500) this.w9.events.push(`${Date.now()} ${m}`);
  }
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
    this.faults = (isolateFaults ??= new Faults(env.FAULTS, Math.random, env.PLAN ?? "plan"));
    this.release = Number(env.RELEASE ?? 1); this.reads = Number(env.READS ?? 1); this.writes = Number(env.WRITES ?? 1);
    this.breakKind = env.BREAK_KIND ?? "";
    const sql = this.sql;
    sql.exec(`CREATE TABLE IF NOT EXISTS state (id INTEGER PRIMARY KEY CHECK (id = 1), grp TEXT, term INTEGER, voted_for TEXT,
              commit_idx INTEGER, applied INTEGER, promise_until INTEGER, led_term INTEGER, transfer TEXT, boots TEXT,
              stepped_down_at INTEGER)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS log (idx INTEGER PRIMARY KEY, term INTEGER NOT NULL, prev TEXT NOT NULL,
              digest TEXT NOT NULL, at INTEGER NOT NULL, body TEXT NOT NULL, packs BLOB)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS req (rid TEXT PRIMARY KEY, idx INTEGER NOT NULL, out TEXT NOT NULL)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS refusal (id INTEGER PRIMARY KEY CHECK (id = 1), no_lead_until INTEGER, cannot_follow TEXT)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS promised (id INTEGER PRIMARY KEY CHECK (id = 1), leader TEXT)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS active (id INTEGER PRIMARY KEY CHECK (id = 1), at INTEGER)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS learners (id INTEGER PRIMARY KEY CHECK (id = 1), slots TEXT)`);
    this.learnerSlots = JSON.parse(sql.exec<{ slots: string }>(`SELECT slots FROM learners WHERE id = 1`).toArray()[0]?.slots ?? "[]");
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
    this.sql.exec(`INSERT OR REPLACE INTO log VALUES (?, ?, ?, ?, ?, ?, ?)`, e.index, e.term, e.prev, e.digest, e.at, JSON.stringify(e.body), blob(e.packs));
    this.last = e.index; this.lastTerm = e.term; this.lastDigest = e.digest;
  }
  /** `packs` false skips the inline pack bytes, for the fold reads that only need the events. */
  private entry(i: number, packs = true): Entry | undefined {
    const mem = this.unpersisted.get(i);
    if (mem) return mem;
    if (!packs) {
      const b = this.bodyCache.get(i);
      if (b) {
        const r = this.sql.exec<{ idx: number; term: number; prev: string; digest: string; at: number }>(`SELECT idx, term, prev, digest, at FROM log WHERE idx = ?`, i).toArray()[0];
        if (r) return { index: r.idx, term: r.term, prev: r.prev, digest: r.digest, at: r.at, body: b, packs: null };
      }
    }
    const r = this.sql.exec<{ idx: number; term: number; prev: string; digest: string; at: number; body: string; packs: ArrayBuffer | null }>(
      `SELECT idx, term, prev, digest, at, body${packs ? ", packs" : ""} FROM log WHERE idx = ?`, i).toArray()[0];
    if (!r) return undefined;
    const body = JSON.parse(r.body) as Body;
    this.bodyCache.set(i, body);
    if (this.bodyCache.size > 16) this.bodyCache.delete(this.bodyCache.keys().next().value!);
    return { index: r.idx, term: r.term, prev: r.prev, digest: r.digest, at: r.at, body, packs: packs && r.packs ? new Uint8Array(r.packs) : null };
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
    this.bodyCache.clear();
    const tail = this.sql.exec<{ idx: number; term: number; digest: string }>(`SELECT idx, term, digest FROM log ORDER BY idx DESC LIMIT 1`).one();
    this.last = tail.idx; this.lastTerm = tail.term; this.lastDigest = tail.digest;
  }
  private kvGet(k: string): string | null {
    return this.sql.exec<{ v: string }>(`SELECT v FROM kv WHERE k = ?`, k).toArray()[0]?.v ?? null;
  }
  /** The fold at `upTo`: applied state in SQLite, plus the events of entries after `applied` up to `upTo`. */
  private foldGet(k: string, upTo: number): string | null {
    for (let i = Math.min(upTo, this.last); i > this.applied; i--) {
      const b = this.entry(i, false)!.body;
      if (b.kind === "events") for (let j = b.events.length - 1; j >= 0; j--) if (b.events[j].k === k) return b.events[j].v;
    }
    return this.kvGet(k);
  }
  /** A request id's committed result: from `req` once applied, else from a committed entry not applied yet. */
  private priorOut(rid: string): { out: Out; idx: number } | undefined {
    const row = this.sql.exec<{ out: string; idx: number }>(`SELECT out, idx FROM req WHERE rid = ?`, rid).toArray()[0];
    if (row) return { out: JSON.parse(row.out), idx: row.idx };
    for (let i = this.applied + 1; i <= this.commitIdx; i++) {
      const b = this.entry(i, false)?.body;
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
        const t = Date.now(), term = this.term;
        void this.ctx.storage.sync().then(() => { if (this.term === term) this.confirmed(Date.now() - t); }, () => {});
      }
      this.write("entry");
      for (const e of [...this.unpersisted.values()].sort((x, y) => x.index - y.index)) {
        this.sql.exec(`INSERT OR REPLACE INTO log VALUES (?, ?, ?, ?, ?, ?, ?)`, e.index, e.term, e.prev, e.digest, e.at, JSON.stringify(e.body), blob(e.packs));
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
      const e = this.entry(this.applied + 1, false)!;
      if (this.breakKind.startsWith("apply:") && e.body.kind === "events" && e.body.events.some((ev) => ev.k.startsWith(this.breakKind.slice(6)))) {
        this.applyErrors++;
        throw new Error(`m0 injected apply failure at ${e.index} (release ${this.release})`);
      }
      if (e.body.kind === "events") {
        for (const ev of e.body.events) this.sql.exec(`INSERT OR REPLACE INTO kv VALUES (?, ?)`, ev.k, ev.v);
        for (const r of e.body.requests) this.sql.exec(`INSERT OR IGNORE INTO req VALUES (?, ?, ?)`, r.rid, e.index, JSON.stringify(r.out));
      }
      this.applied = e.index;
      this.feedApplied(e);
    }
  }

  /** A storage write the fault plan may fail. A failed write resets the object, as an unconfirmed write does. Called
   *  right before each write, with no await between it and the write. */
  private write(what: string): void {
    if (this.faults.rollNow("write", `${this.self}:${what}`)) this.ctx.abort(`m0 injected fault: write ${this.self}:${what}`);
  }
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
  private health(site: SiteKey, ok: boolean): void {
    if (this.role !== "leader") return;
    // A follower not yet heard from in this leadership is one the quorum commits without; its failures say nothing
    // about this leader's site (src/raft/member.ts, M0-WP12).
    if (ok) this.heard.add(site); else if (!this.heard.has(site)) return;
    this.callHealth.push([Date.now(), site, ok]);
    this.checkHealth();
  }
  private confirmed(ms: number): void {
    if (this.role !== "leader") return;
    if (this.w9.confirmMs.length < 50_000) this.w9.confirmMs.push(ms);
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
    // Outside the design: a group named "hold-..." never steps down, so B0-31 can measure an apac leader's steady state.
    if (variant(this.group, "hold") !== null) return;
    const rates = this.others().map((f) => {
      const c = this.callHealth.filter(([, s]) => s === f);
      return c.length >= 10 ? c.filter(([, , ok]) => !ok).length / c.length : 0;
    });
    const confirms = this.confirmHealth.map(([, ms]) => ms).sort((a, b) => a - b);
    // As in M0-WP2: p99 over the window's confirmations, so under 100 of them it is the largest. Outside the design, a
    // group named "sdN-..." needs N confirmations over the line before the confirm rule trips (M0-WP9 finding).
    // Fourth pass: the design's rule, at least STEP_DOWN.confirmSlowMin (6) confirmations over the line; "sdN-" still
    // overrides N for comparison with earlier passes.
    const need = variant(this.group, "sd") ?? STEP_DOWN.confirmSlowMin;
    const p99 = confirms.length ? confirms[Math.floor(confirms.length * 0.99)] : 0;
    const slowN = confirms.filter((ms) => ms > STEP_DOWN.confirmP99Ms).length;
    // Eighth pass, adopted commit_fail_min: at least 3 failed calls to every follower in the window, as well as the rate.
    const failed = this.others().map((f) => this.callHealth.filter(([, x, ok]) => x === f && !ok).length);
    const enough = Math.min(...failed) >= STEP_DOWN.commitFailMin;
    if (Math.min(...rates) > STEP_DOWN.commitErrors && !enough && Date.now() >= this.healthHoldUntil && this.min3Notes++ < 50)
      this.event(`commit errors ${rates.map((r) => r.toFixed(2)).join("/")} but failed calls ${failed.join("/")}: under commit_fail_min, no trip`);
    const why = Math.min(...rates) > STEP_DOWN.commitErrors && enough ? `commit errors ${rates.map((r) => r.toFixed(2)).join("/")}, failed ${failed.join("/")}`
      : p99 > STEP_DOWN.confirmP99Ms && slowN >= need ? `confirm p99 ${p99} ms, ${slowN} of ${confirms.length} over ${STEP_DOWN.confirmP99Ms} ms` : null;
    if (!why || Date.now() < this.healthHoldUntil) return;
    // Transfer target, as src/raft/member.ts (M0-WP4's revision of item 34, landed 23:11Z on 2026-10-30, after the
    // seventh pass's runs had started; none of them tripped STEP_DOWN): a follower that may lead (undrained, able to
    // follow) and is outside RESET_QUARANTINE of its own `stepped_down_at`; on a confirm trip, one whose calls from this
    // leader fail under `commit_errors` in the window; on a commit-error trip, one whose calls from the other follower
    // fail under `commit_errors` by that follower's `linkErrors` probe, and a probe that cannot run disqualifies it.
    // Then the one whose calls failed least, then the most caught up; hold if none qualifies.
    const m = modesView?.modes ?? NO_MODES;
    const quarantined = (s: SiteKey) => (this.peerSteppedDownAt[s] ?? 0) > Date.now() - RESET_QUARANTINE;
    const rate = (s: SiteKey) => rates[this.others().indexOf(s)];
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
          const other = this.others().find((x) => x !== t)!;
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
  private async modes(): Promise<Modes> {
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
        else if (modesView) modesView.at = Date.now();
      })().finally(() => { modesRefresh = null; });
    }
    if (!modesView && modesRefresh) await modesRefresh;
    return modesView?.modes ?? NO_MODES;
  }
  /** Served by a `release` group member: the site modes in its applied state. */
  async modeView(): Promise<{ applied: number; modes: Modes }> {
    const v: Record<string, string | null> = {};
    for (const k of MODE_KEYS) v[k] = this.kvGet(k);
    return { applied: this.applied, modes: modesFrom(v) };
  }
  /** Why this member must not lead now: its site is drained or under the 1% lead limit, or `no_lead_until` holds. */
  private async leadBar(): Promise<string | null> {
    if (this.noLeadUntil === this.release) return `no_lead_until ${this.noLeadUntil}`;
    const m = await this.modes();
    return mayLead(this.self, this.group, m) ? null : m.drained.includes(this.self) ? "drained" : "lead limit";
  }
  /** A leader that must not lead hands off within one lease to an undrained follower that can follow (design/DESIGN.md,
   *  `transfer` and `SiteMode`). Checked on requests, at most once a second; groups are request-driven. */
  private async maybeHandOff(): Promise<void> {
    if (this.role !== "leader" || !this.ready || this.transferring || this.steppingDown || Date.now() < this.drainCheckAt) return;
    this.drainCheckAt = Date.now() + 1_000;
    const why = await this.leadBar();
    if (!why || this.role !== "leader" || this.transferring) return;
    const m = await this.modes();
    const to = this.others().filter((s) => mayLead(s, this.group, m) && this.cannotFollow[s] === undefined)
      .sort((x, y) => (this.match[y] ?? 0) - (this.match[x] ?? 0))[0];
    if (!to) { this.event(`must not lead (${why}) but no follower can take over`); return; }
    this.event(`handing off (${why}) to ${to}`);
    const r = await this.transfer(this.group, to, false);
    this.event(`hand-off: ${r}`);
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
    this.lanes = { Human: [], MergeQueue: [], Bot: [] }; this.agentQ.clear(); this.agentOrder = [];
    this.agentQueued = 0; this.agentQueuedBytes = 0; this.agentInlineQueued = 0;
    this.ridSeq.clear(); this.pending = [];
    if (this.leader === this.self) this.leader = null;
  }

  private becomeLeader(resumed: boolean): void {
    this.w9.leaderStarts++;
    this.answerHeldPolls();
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
    // Sixth pass, adopted item 35: `hot` comes from the log, so the per-entry budget applies from the first entry. Each
    // compare-and-set in the entries committed within the hot window counts as one push at its entry's time.
    const now = Date.now();
    this.pushTimes = [];
    for (let i = this.commitIdx, n = 0; i > 0 && n < 400; i--, n++) {
      const e = this.entry(i, false);
      if (!e || e.at < now - 5_000) break;
      if (e.body.kind === "events") for (const ev of e.body.events) if (ev.expect !== undefined) this.pushTimes.push(e.at);
    }
    this.event(`hot from log: ${this.pushTimes.length} pushes in the last 5 s, hot ${this.pushTimes.length / 5 > HOT_PUSHES}`);
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
      this.answerHeldPolls();
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
        let bytes = 0;
        for (let i = from; i <= Math.min(this.last, from + CATCHUP_MAX - 1) && (bytes < CATCHUP_BYTES || !entries.length); i++) {
          const e = this.entry(i)!; entries.push(e); bytes += entryBytes(e);
        }
        const prev = from - 1;
        const req: AppendReq = { term, leader: this.self, prevIndex: prev, prevTerm: this.logTerm(prev) ?? 0,
          prevDigest: prev === 0 ? ZERO : this.entry(prev, false)?.digest ?? ZERO, entries,
          commit: this.commitIdx, transfer: this.transferring?.id ?? null };
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
            if (this.bytesOut.length < 5000) this.bytesOut.push(entries.reduce((a, e) => a + entryBytes(e), 0));
          }
        } catch (err) {
          this.health(site, false);
          recordCall(this.env.HEALTH, this.self, site, false);
          if (failures === 0) this.event(`append to ${site} failed: ${String(err).slice(0, 60)}`);
          failures++;
          failingSince ||= sentAt;
          this.heartbeat[site] = true;
          // Retry COMMIT_RETRIES times at once, then keep trying while the round still needs this follower.
          if (failures > COMMIT_RETRIES) {
            if (Date.now() - failingSince > 30_000 || (this.commitIdx >= this.last && !this.roundWaiters.length)) break;
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
    } catch {
      this.health(site, false);
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
    while (this.pending.length && this.commitIdx >= this.pending[0].index) {
      const e = this.pending.shift()!;
      const now = Date.now();
      this.entrySizes.push(e.seqs.length);
      this.entryMs.push(now - e.started);
      this.roundEwma = 0.8 * this.roundEwma + 0.2 * (now - e.started);
      this.fit.push([e.bytes, now - e.started]); if (this.fit.length > 300) this.fit.shift();
      const tl = this.timeline.get(e.index); if (tl) tl[5] = now;
      for (const [rid, seq] of this.ridSeq) if (e.seqs.includes(seq)) this.ridSeq.delete(rid);
      for (const seq of e.seqs) {
        const w = this.tickets.get(seq);
        if (w && !w.result) {
          this.commitLat.push(now - w.at);
          const out = e.outs.get(seq)!, waited = e.index - w.admitLast;
          this.w9.committed(w.cls, now - w.at, waited);
          w.result = { out, commitMs: now - w.at, entry: e.index, cls: w.cls, waited }; w.resolve(w.result);
        }
      }
    }
    for (const slot of this.learnerSlots) void this.feedLearner(slot);
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

  // ---- M0-WP9: admission ----

  /** Commands a second this group carries: measured entries/s (one entry in flight, so 1 / round) times commands per
   *  entry at the offered byte mix (design/DESIGN.md, `AdmitClass`). A group named "capN-..." uses N instead. */
  /** The round grows with entry bytes (B0-02: p50 739 ms at 0.5 to 1.8 MB entries), so commands per entry at the
   *  current byte mix are priced at the round a full entry of them would take: a least-squares fit of round against
   *  entry bytes over the last 300 entries, with a prior slope until the entries spread over at least 300 KB. */
  private fit: [number, number][] = [];
  private roundAt(bytes: number): number {
    const xs = this.fit;
    const prior = 0.0003; // ms per byte [measured in M0-WP2: an echo between members took 96 ms empty and 533 ms at 1.5 MB]
    if (xs.length < 20) return this.roundEwma + prior * bytes;
    const n = xs.length, mx = xs.reduce((a, x) => a + x[0], 0) / n, my = xs.reduce((a, x) => a + x[1], 0) / n;
    const sxx = xs.reduce((a, x) => a + (x[0] - mx) ** 2, 0), sxy = xs.reduce((a, x) => a + (x[0] - mx) * (x[1] - my), 0);
    const spread = Math.max(...xs.map((x) => x[0])) - Math.min(...xs.map((x) => x[0]));
    const k = spread >= 300_000 && sxx > 0 ? Math.max(0, sxy / sxx) : prior;
    return Math.max(50, my - k * mx) + k * bytes;
  }
  private capAt = 0;
  private capCached = 300;
  private capacity(): number {
    const fixed = variant(this.group, "cap");
    if (fixed) return fixed;
    if (Date.now() - this.capAt < 1_000) return this.capCached;
    this.capAt = Date.now();
    const perEntry = Math.min(BATCH_MAX, ENTRY_MAX / Math.max(200, this.cmdBytesEwma));
    this.capCached = (1000 * perEntry) / this.roundAt(perEntry * this.cmdBytesEwma);
    return this.capCached;
  }
  private queuedCount(): number { return this.lanes.Human.length + this.lanes.MergeQueue.length + this.lanes.Bot.length + this.agentQueued; }
  private hints(): Hints {
    const now = Date.now();
    this.pushTimes = this.pushTimes.filter((t) => t > now - 5_000);
    // The design does not define pressure. Here: more agent bytes queued than one entry holds, or more agent commands
    // than the bucket admits in PRESSURE_AGE.
    return { hot: this.pushTimes.length / 5 > HOT_PUSHES, placeOk: variant(this.group, "nop") === null,
      pressure: this.agentQueuedBytes > ENTRY_MAX || this.agentQueued > (AGENT_SHARE * this.capacity() * PRESSURE_AGE) / 1000 };
  }
  private retryAfter(): number {
    return Math.max(1, Math.ceil(this.agentQueued / Math.max(1, AGENT_SHARE * this.capacity())) + 1);
  }
  private takeAgent(): Queued | undefined {
    while (this.agentOrder.length) {
      const actor = this.agentOrder.shift()!;
      const q = this.agentQ.get(actor)!;
      const c = q.shift()!;
      if (q.length) this.agentOrder.push(actor); else this.agentQ.delete(actor);
      this.agentQueued--; this.agentQueuedBytes -= c.bytes; this.agentInlineQueued -= c.inl;
      return c;
    }
    return undefined;
  }
  private peekAgent(): Queued | undefined { const a = this.agentOrder[0]; return a === undefined ? undefined : this.agentQ.get(a)![0]; }
  /** AGENT_WAIT: an agent command still queued after it is answered with the refusal text and never enters an entry. */
  private expiredAt = 0;
  private expireAgents(): void {
    const now = Date.now(), cutoff = now - AGENT_WAIT;
    if (now - this.expiredAt < 250) return;
    this.expiredAt = now;
    let emptied = false;
    for (const actor of this.agentOrder) {
      const q = this.agentQ.get(actor)!;
      while (q.length && q[0].at < cutoff) {
        const c = q.shift()!;
        this.agentQueued--; this.agentQueuedBytes -= c.bytes; this.agentInlineQueued -= c.inl;
        this.refuse(c, "wait");
      }
      if (!q.length) { this.agentQ.delete(actor); emptied = true; }
    }
    if (emptied) this.agentOrder = this.agentOrder.filter((a) => this.agentQ.has(a));
  }
  private refuse(c: Queued, why: "wait" | "queue"): void {
    const text = refusalText(c.meta.kind, c.op.key, this.retryAfter());
    this.w9.refusal(why, text);
    this.ridSeq.delete(c.rid);
    const w = this.tickets.get(c.seq);
    if (w && !w.result) { w.result = { out: { ok: false, value: text }, commitMs: Date.now() - w.at, entry: -1, cls: c.cls, refused: true }; w.resolve(w.result); }
  }
  private tick(): void {
    if (this.pumpTimer || !this.queuedCount()) return;
    this.pumpTimer = true;
    setTimeout(() => { this.pumpTimer = false; this.pump(); this.tick(); }, 50);
  }
  private lastEntryAt = 0;
  /** INLINE_MAX_HOT, or N KiB in a group whose name holds "ihN-" (an M0-WP9 variant, outside the design). */
  private get inlineMaxHot(): number { const n = variant(this.group, "ih"); return n !== null ? n << 10 : INLINE_MAX_HOT; }
  private sampledAt = 0;

  /** Group commit: one entry in flight. Each command's decision sees the fold plus earlier commands of its entry. The
   *  entry's commands are chosen by admission: Human and MergeQueue, then Agent up to AGENT_FLOOR of capacity, then
   *  Bot, then Agent up to the AGENT_SHARE token bucket, all within ENTRY_MAX bytes and BATCH_MAX commands. */
  private pump(): void {
    if (this.role !== "leader" || !this.ready || this.pending.length >= this.pipeDepth || this.transferring) return;
    if (this.noLeadUntil === this.release) return; // handing off; endLeadership answers the queue NotLeader
    this.expireAgents();
    const now = Date.now();
    const cap = this.capacity(), rate = AGENT_SHARE * cap;
    this.tokens = Math.min(rate, this.tokens + (rate * (now - this.tokensAt)) / 1000); this.tokensAt = now;
    if (now - this.sampledAt >= 1000) {
      this.sampledAt = now;
      const reqs = Object.values(this.reqs).reduce((a, b) => a + b, 0);
      this.w9.second([now, Math.round(cap), Math.round(this.tokens), this.agentQueued, this.agentQueuedBytes, this.queuedCount() - this.agentQueued,
        Math.round(this.roundEwma), reqs, this.reqs.outcome ?? 0, this.reqs.feed ?? 0]);
    }
    if (!this.queuedCount()) return;
    const batch: Queued[] = [];
    let bytes = 0;
    const fits = (c: Queued | undefined) => !!c && batch.length < BATCH_MAX && (bytes + c.bytes <= ENTRY_MAX || !batch.length);
    // Seventh pass, adopted edit 38: AGENT_INLINE_ENTRY on every group, hot or not, also at the cut. One agent pack over
    // the budget still rides alone in an entry when the queue holds nothing else (it was admitted under INLINE_MAX).
    let agentInl = 0;
    const fitsAgent = (c: Queued | undefined) => fits(c) && (!agentInl || agentInl + c!.inl <= AGENT_INLINE_ENTRY);
    const take = (c: Queued) => { batch.push(c); bytes += c.bytes; agentInl += c.inl; };
    for (const lane of [this.lanes.Human, this.lanes.MergeQueue]) while (fits(lane[0])) take(lane.shift()!);
    const floor = Math.floor((AGENT_FLOOR * cap * Math.max(this.roundEwma, now - this.lastEntryAt)) / 1000);
    for (let n = 0; n < floor && this.tokens >= 1 && fitsAgent(this.peekAgent()); n++) { take(this.takeAgent()!); this.tokens--; }
    while (fits(this.lanes.Bot[0])) take(this.lanes.Bot.shift()!);
    while (this.tokens >= 1 && fitsAgent(this.peekAgent())) { take(this.takeAgent()!); this.tokens--; }
    if (!batch.length) { this.tick(); return; }
    this.lastEntryAt = now;
    const overlay = new Map<string, string | null>();
    const events: Ev[] = [], requests: { rid: string; out: Out }[] = [], seqs: number[] = [], outs = new Map<number, Out>();
    const packs: Uint8Array[] = [];
    const counts: Record<AdmitClass, number> = { Human: 0, MergeQueue: 0, Bot: 0, Agent: 0 };
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
        const ev: Ev = { k: c.op.key, v: c.op.value, expect: c.op.expect };
        if (c.op.pack?.byteLength) { ev.pack = c.op.pack.byteLength; packs.push(c.op.pack); }
        if (c.op.p) ev.p = c.op.p;
        events.push(ev); overlay.set(c.op.key, c.op.value); out = { ok: true };
      } else if (c.op.kind === "cas") { out = { ok: false, value: "stale" }; this.w9.staleBy(c.cls, c.meta.kind); }
      else out = { ok: false };
      counts[c.cls]++;
      requests.push({ rid: c.rid, out }); seqs.push(c.seq); outs.set(c.seq, out);
    }
    if (!seqs.length) return this.pump();
    // The new record shape (`WRITES` 2) is written only by the rare command it belongs to: keys starting "new:".
    const body: Body = this.writes > 1 && events.some((ev) => ev.k.startsWith("new:"))
      ? { kind: "events", events, requests, format: this.writes } : { kind: "events", events, requests };
    let joined: Uint8Array | null = null;
    if (packs.length) {
      joined = new Uint8Array(packs.reduce((a, x) => a + x.byteLength, 0));
      let o = 0; for (const x of packs) { joined.set(x, o); o += x.byteLength; }
    }
    const e = { term: this.term, index: this.last + 1, prev: this.lastDigest, at: Date.now(), body, packs: joined };
    this.appendLocal({ ...e, digest: digestOf(e) });
    this.w9.entry([e.at, seqs.length, bytes, joined?.byteLength ?? 0, counts.Human, counts.MergeQueue, counts.Bot, counts.Agent, agentInl]);
    this.timeline.set(e.index, [e.at, 0, 0, 0, 0, 0]);
    if (this.timeline.size > 2000) this.timeline.delete(this.timeline.keys().next().value!);
    this.pending.push({ index: e.index, started: e.at, seqs, outs, bytes: entryBytes({ ...e, digest: "" }) });
    this.maxPending = Math.max(this.maxPending, this.pending.length);
    for (const s of this.others()) void (this.pipeDepth > 1 ? this.sendOne(s, e.index) : this.replicate(s));
    this.scheduleFlush();
  }

  /** Followers check entry k+1 against their fold at k before storing it (design/DESIGN.md, `verify`). */
  private verify(e: Entry): string | null {
    if (e.prev !== this.lastDigest) return `prev digest mismatch at ${e.index}`;
    if (e.digest !== digestOf({ term: e.term, index: e.index, prev: e.prev, at: e.at, body: e.body, packs: e.packs })) return `bad digest at ${e.index}`;
    if (e.body.kind !== "events") return null;
    if ((e.body.format ?? 1) > this.reads) return `format ${e.body.format} at ${e.index} is unreadable by release ${this.release}`;
    if (this.breakKind === "refuse") return `injected refusal at ${e.index} by release ${this.release}`;
    const running = new Map<string, string | null>();
    const valueBefore = (k: string): string | null => {
      if (running.has(k)) return running.get(k)!;
      for (let i = this.last; i > this.applied; i--) {
        const b = this.entry(i, false)!.body;
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
    this.count("append");
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
    if (req.prevIndex > this.last) { this.gap = true; this.answerHeldPolls(); this.save(); return no({ promise }); }
    // Entries match by digest, not term: a restarted leader that resumes `led_term` may have lost an entry it sent
    // before its own write was confirmed, and then sends a different entry at the same (term, index). Such an entry
    // was never committed, because a commit is revealed only after the leader's own write is confirmed.
    if (req.prevIndex > 0 && this.entry(req.prevIndex, false)?.digest !== req.prevDigest) {
      this.truncateFrom(req.prevIndex); this.gap = true; this.answerHeldPolls(); this.save(); return no({ promise, last: this.last });
    }
    for (const e of req.entries) {
      if (e.index <= this.last) {
        if (this.entry(e.index, false)?.digest === e.digest) continue;
        this.truncateFrom(e.index);
      }
      try { this.applyUpTo(Math.min(req.commit, e.index - 1)); }
      catch (err) { this.refusedByMe++; this.save(); return no({ promise, refused: String(err).slice(0, 120), refusedAt: e.index, release: this.release }); }
      const refused = this.verify(e);
      if (refused) { this.refusedByMe++; this.save(); return no({ promise, refused, refusedAt: e.index, release: this.release }); }
      this.storeEntry(e);
    }
    this.gap = false;
    this.commitIdx = Math.max(this.commitIdx, Math.min(req.commit, req.prevIndex + req.entries.length));
    try { this.applyUpTo(this.commitIdx); } catch (err) { this.event(`apply failed: ${String(err).slice(0, 80)}`); }
    this.save();
    // The output gate holds the reply until these writes are confirmed anyway; awaiting sync() only makes that wait
    // visible in `serveMs`.
    const t1 = Date.now();
    if (req.entries.length) await this.ctx.storage.sync();
    this.confirmSplit.push([t1 - now, Date.now() - t1]);
    return { term: this.term, ok: true, match: req.prevIndex + req.entries.length, last: this.last, promise, serveMs: Date.now() - now,
      release: this.release, steppedDownAt: this.steppedDownAt };
  }

  async vote(group: string, req: VoteReq, pre: boolean): Promise<VoteRes> {
    this.setGroup(group);
    this.count("vote");
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
    this.w9.suspects++;
    this.count("suspect");
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

  /** `meta` is the edge's classification of the command (M0-WP9); the leader may demote it to Agent. */
  async commit(group: string, rid: string, op: RaftOp, meta: Meta = { cls: "Human", actor: "-", kind: "side" }): Promise<CommitRes> {
    this.setGroup(group);
    this.count("commit");
    if (!(await this.canLead())) return { notLeader: { term: this.term, leader: this.leader } };
    void this.maybeHandOff();
    const t0 = Date.now();
    const prior = this.priorOut(rid);
    if (Date.now() - t0 > 50 && this.slowNotes++ < 20) this.event(`commit: priorOut took ${Date.now() - t0} ms`);
    if (prior) return { done: prior.out };
    // A loop that gave up on an unreachable follower restarts on the next command.
    if (this.pending.length) for (const s of this.others()) void this.replicate(s);
    const joined = this.ridSeq.get(rid);
    if (joined !== undefined) return this.hold({ term: this.term, boot: this.boot, seq: joined }, this.hints());
    // Admission class: the edge's, demoted to Agent when the actor passes DEMOTE_PER_MIN writes a minute on this group.
    const now = Date.now();
    let cls = meta.cls;
    const times = (this.actorWrites.get(meta.actor) ?? []).filter((t) => t > now - 60_000);
    times.push(now); this.actorWrites.set(meta.actor, times);
    if (this.actorWrites.size > 20_000) for (const [a, ts] of this.actorWrites) if (ts[ts.length - 1] < now - 60_000) this.actorWrites.delete(a);
    if ((cls === "Human" || cls === "Bot") && times.length > DEMOTE_PER_MIN) { cls = "Agent"; this.w9.demoted++; }
    const packLen = op.kind === "cas" ? op.pack?.byteLength ?? 0 : 0;
    const bytes = 200 + op.key.length + ("value" in op ? op.value.length : 0) + packLen;
    const hints = this.hints();
    // AGENT_QUEUE_MAX refuses before staging. Then the hot inline cap, and under pressure only `PackRef::Stored` agent
    // pushes queue: the edge stages the pack first and sends again.
    const full = (b: number) => this.agentQueued >= AGENT_QUEUE_MAX.cmds || this.agentQueuedBytes + b > AGENT_QUEUE_MAX.bytes;
    // While placement fails the hot cap yields: packs up to INLINE_MAX stay inline (design/DESIGN.md, INLINE_MAX_HOT).
    const stage = hints.placeOk && packLen > 0 && ((hints.hot && packLen > this.inlineMaxHot) || (cls === "Agent" && hints.pressure));
    // Third pass, the adopted outage rule: while placement fails, an agent push over INLINE_MAX_HOT gets the agent
    // refusal at once and is never queued; Human and MergeQueue pushes keep INLINE_MAX.
    if (!hints.placeOk && cls === "Agent" && packLen > this.inlineMaxHot) {
      const text = refusalText(meta.kind, op.key, this.retryAfter());
      this.w9.refusal("outage", text);
      return { refused: text, hints };
    }
    // Fifth pass, the adopted per-entry budget: on a hot group an entry carries at most AGENT_INLINE_ENTRY of Agent-class
    // inline pack bytes. Every entry is cut from the queue, so an agent pack that would take the queued Agent inline
    // bytes past it is staged as `Stored` (refused at once while placement fails). Human and MergeQueue packs don't count.
    // Seventh pass, adopted edit 38: on every group, not only hot ones.
    const overBudget = cls === "Agent" && packLen > 0 && !stage && this.agentInlineQueued + packLen > AGENT_INLINE_ENTRY;
    if (overBudget && !hints.placeOk) {
      const text = refusalText(meta.kind, op.key, this.retryAfter());
      this.w9.refusal("outage-budget", text);
      return { refused: text, hints };
    }
    if (overBudget) { this.w9.budgetStaged++; this.w9.staged++; return { stage: true, hints }; }
    if (cls === "Agent" && full(stage ? bytes - packLen : bytes)) {
      const text = refusalText(meta.kind, op.key, this.retryAfter());
      this.w9.refusal("queue", text); this.w9.queueRefused++;
      return { refused: text, hints };
    }
    if (stage) { this.w9.staged++; return { stage: true, hints }; }
    this.cmdBytesEwma = 0.98 * this.cmdBytesEwma + 0.02 * bytes;
    if (meta.kind === "push") this.pushTimes.push(now);
    const seq = ++this.seq;
    let resolve!: (v: OutcomeRes) => void;
    const promise = new Promise<OutcomeRes>((r) => (resolve = r));
    this.tickets.set(seq, { resolve, promise, at: now, cls, admitLast: this.last });
    if (this.tickets.size > 2_000) for (const [s, w] of this.tickets) if (w.result && now - w.at > 60_000) this.tickets.delete(s);
    this.ridSeq.set(rid, seq);
    const q: Queued = { rid, op, seq, meta, cls, bytes, at: now, inl: cls === "Agent" ? packLen : 0 };
    if (cls === "Agent") {
      const lane = this.agentQ.get(meta.actor);
      if (lane) lane.push(q); else { this.agentQ.set(meta.actor, [q]); this.agentOrder.push(meta.actor); }
      this.agentQueued++; this.agentQueuedBytes += bytes; this.agentInlineQueued += q.inl;
      this.w9.agentQueuedPeak = Math.max(this.w9.agentQueuedPeak, this.agentQueued);
      this.w9.agentBytesPeak = Math.max(this.w9.agentBytesPeak, this.agentQueuedBytes);
    } else this.lanes[cls].push(q);
    this.pump();
    this.tick();
    return this.hold({ term: this.term, boot: this.boot, seq }, hints);
  }

  /** Sixth pass, adopted (fold `outcome` into `commit`): the commit call waits for its ticket's result, which only the
   *  commit-advance path (quorum rule), an AGENT_WAIT or queue refusal, or the end of leadership resolves, and answers
   *  with it. After COMMIT_HOLD it answers the ticket instead, and the edge falls back to one `outcome` call. */
  private async hold(ticket: Ticket, hints: Hints): Promise<CommitRes> {
    const w = this.tickets.get(ticket.seq);
    if (!w) return { admitted: ticket, hints };
    let timer: ReturnType<typeof setTimeout> | undefined;
    this.held++; this.w9.heldPeak = Math.max(this.w9.heldPeak, this.held);
    try {
      const res = await Promise.race([w.promise, new Promise<null>((r) => { timer = setTimeout(() => r(null), COMMIT_HOLD); })]);
      if (res === null) { this.w9.holdExpired++; return { admitted: ticket, hints }; }
      this.tickets.delete(ticket.seq);
      if ("notLeader" in res) return { notLeader: res.notLeader };
      return { result: res, hints };
    } finally { if (timer !== undefined) clearTimeout(timer); this.held--; }
  }
  private held = 0;
  private min3Notes = 0;

  /** Sixth pass: the fallback for an edge whose held `commit` call timed out and so holds no ticket: the outcome of
   *  request `rid`, from the request rows if it committed, else by waiting on its ticket here. `unknown` means this
   *  leader never admitted it, and the edge sends `commit` again with the same rid. */
  async outcomeRid(group: string, rid: string): Promise<OutcomeRes | { unknown: true }> {
    this.setGroup(group);
    this.count("outcome");
    const prior = this.priorOut(rid);
    if (prior) return { out: prior.out, commitMs: -1, entry: prior.idx };
    if (this.role !== "leader") return { notLeader: { term: this.term, leader: this.leader } };
    const seq = this.ridSeq.get(rid);
    if (seq === undefined) return { unknown: true };
    return this.outcome(group, { term: this.term, boot: this.boot, seq }, false);
  }

  async outcome(group: string, ticket: Ticket, counted = true): Promise<OutcomeRes> {
    this.setGroup(group);
    if (counted) this.count("outcome");
    const w = this.tickets.get(ticket.seq);
    if (ticket.term !== this.term || ticket.boot !== this.boot || !w) return { notLeader: { term: this.term, leader: this.role === "leader" ? this.self : this.leader } };
    // Fourth pass: the timer is cleared when the outcome arrives. Left running, each call held a 35 s timeout, and at about
    // 255 commands/s a leader passed the isolate's active-timeout limit (B0-33 p4r3: QuotaExceededError on every call).
    let timer: ReturnType<typeof setTimeout> | undefined;
    const res = await Promise.race([w.promise, new Promise<OutcomeRes>((r) => { timer = setTimeout(() => r({ notLeader: { term: this.term, leader: null } }), AGENT_WAIT + 5_000); })]);
    if (timer !== undefined) clearTimeout(timer);
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

  // ---- M0-WP9: activity feed and leader-fed learners ----

  /** design/AGENT-LAYER.md, "Activity feed": a member follows while its last AppendEntries left no gap. A voter that
   *  is not the leader and knows one, or a learner its leader has fed, with no gap since. (No `InstallSnapshot` here.) */
  private gap = true;
  private following(): boolean {
    if (this.role !== "follower" || this.gap) return false;
    return this.isLearner ? this.lastFedAt > 0 : this.leader !== null && this.leader !== this.self;
  }
  private feedApplied(e: Entry): void {
    if (e.body.kind !== "events" || !e.body.events.length) return;
    const r = { index: e.index, paths: e.body.events.flatMap((ev) => (ev.p ? [ev.p] : [])), n: e.body.events.length };
    this.ring.push(r);
    if (this.ring.length > FEED_RING) this.ring.splice(0, this.ring.length - FEED_RING);
    for (const w of this.feedWaiters) {
      if (w.matched || !(w.paths === null || r.paths.some((p) => w.paths!.some((q) => p.startsWith(q))))) continue;
      w.matched = true;
      setTimeout(w.resolve, w.batch);
    }
  }
  /** A member that stops following answers its held polls at once, with their cursor and no events. */
  private answerHeldPolls(): void { for (const w of [...this.feedWaiters]) w.resolve(); }
  /** `GET .../activity?cursor=&wait=&paths=` on one member (design/AGENT-LAYER.md, Shape, "Activity feed"). Never on
   *  the leader. Answers from the applied log with no read-index call. `paths` are prefixes here, not globs. */
  async feed(group: string, cursor: number, waitMs: number, paths: string[] | null): Promise<FeedRes> {
    this.setGroup(group);
    this.count("feed");
    if (this.role === "leader") { this.w9.feedOnLeader++; return { notServing: "leader" }; }
    if (!this.following()) { this.w9.feedRefused++; return { notServing: "not following" }; }
    if (this.feedWaiters.size >= FEED_HELD_MAX) { this.w9.feedFull++; return { full: true }; }
    const t0 = Date.now();
    const matches = (r: { paths: string[] }) => paths === null || r.paths.some((p) => paths.some((q) => p.startsWith(q)));
    const from = cursor < 0 ? this.applied : cursor;
    if (waitMs > 0) {
      await new Promise<void>((resolve) => {
        const w: FeedWaiter = { paths, matched: false, batch: paths ? FEED_BATCH : FEED_BATCH_ALL, resolve: () => {} };
        w.resolve = () => { if (this.feedWaiters.delete(w)) { this.w9.feedHeld = this.feedWaiters.size; resolve(); } };
        this.feedWaiters.add(w);
        this.w9.feedHeld = this.feedWaiters.size;
        this.w9.feedHeldPeak = Math.max(this.w9.feedHeldPeak, this.feedWaiters.size);
        if (this.ring.some((r) => r.index > from && matches(r))) { w.matched = true; setTimeout(w.resolve, w.batch); }
        setTimeout(w.resolve, Math.min(waitMs, FEED_WAIT_MAX));
      });
    }
    const events = this.ring.filter((r) => r.index > from && matches(r)).reduce((a, r) => a + r.n, 0);
    this.w9.feedServed++; this.w9.feedEvents += events;
    return { cursor: this.applied, events, site: this.self, heldMs: Date.now() - t0 };
  }

  /** Sets the group's learner slots on this voter; its leader then sends committed entries to each. */
  async setLearners(group: string, slots: number[]): Promise<void> {
    this.setGroup(group);
    this.learnerSlots = slots;
    this.sql.exec(`INSERT OR REPLACE INTO learners VALUES (1, ?)`, JSON.stringify(slots));
  }
  private learnerStub(slot: number): DurableObjectStub<RaftMember> {
    const site = SITES[slot % 3], ns = this.env[`MEMBER_${site.toUpperCase()}` as "MEMBER_A"];
    return ns.get(ns.idFromName(learnerName(this.group, slot)), { locationHint: hintFor(this.group, site) });
  }
  /** Leader: sends committed entries to a learner, one call in flight per learner. Learners never count to a quorum. */
  private async feedLearner(slot: number): Promise<void> {
    if (this.lInflight.has(slot) || this.role !== "leader") return;
    this.lInflight.add(slot);
    try {
      for (let n = 0; n < 50 && this.role === "leader"; n++) {
        const known = this.lNext.get(slot);
        if (known === undefined) {
          const r = await call(() => this.learnerStub(slot).learnerAppend(this.group, []), CALL_DEADLINE * 3, 0);
          this.lNext.set(slot, r.last + 1);
          continue;
        }
        const from = known;
        if (from > this.commitIdx) break;
        const entries: Entry[] = [];
        let bytes = 0;
        for (let i = from; i <= this.commitIdx && i < from + CATCHUP_MAX && (bytes < CATCHUP_BYTES || !entries.length); i++) {
          const e = this.entry(i)!; entries.push(e); bytes += entryBytes(e);
        }
        const r = await call(() => this.learnerStub(slot).learnerAppend(this.group, entries), CALL_DEADLINE * 3, 0);
        this.lNext.set(slot, r.last + 1);
      }
    } catch { /* retried on the next commit */ } finally { this.lInflight.delete(slot); }
  }
  /** Learner: stores committed entries that extend its chain, applies them, and serves feed polls from them. */
  async learnerAppend(group: string, entries: Entry[]): Promise<{ last: number }> {
    this.setGroup(group);
    this.count("learnerAppend");
    this.isLearner = true; this.lastFedAt = Date.now(); this.w9.learnerAppends++;
    const before = this.last;
    this.absorb(entries);
    this.gap = entries.length > 0 && this.last < before + entries.length && entries[entries.length - 1].index > this.last;
    if (this.gap) this.answerHeldPolls();
    this.commitIdx = Math.max(this.commitIdx, this.last);
    this.applyUpTo(this.commitIdx);
    this.save();
    return { last: this.last };
  }
  /** M0-WP9 counters; `reset` clears them and the request counts. */
  async w9stats(reset: boolean): Promise<Record<string, unknown>> {
    const out = { ...this.w9, site: this.self, role: this.role, term: this.term, boot: this.boot, isLearner: this.isLearner,
      reqs: { ...this.reqs }, reqsSeconds: (Date.now() - this.reqsSince) / 1000, applied: this.applied, last: this.last,
      roundEwma: this.roundEwma, cmdBytesEwma: this.cmdBytesEwma, capacity: this.capacity(), learners: this.learnerSlots };
    if (reset) { this.w9 = Member9Stats.empty(); this.reqs = {}; this.reqsSince = Date.now(); this.commitLat = []; this.entrySizes = []; this.entryMs = []; }
    return out;
  }

  // ---- RPC: test control ----

  /** Raft leadership transfer. `abandon` stops after catching `to` up, as an abandoned transfer does. */
  async transfer(group: string, to: SiteKey, abandon: boolean): Promise<string> {
    this.setGroup(group);
    if (this.role !== "leader" || to === this.self) return "not leader";
    const id = crypto.randomUUID();
    this.transferring = { to, id };
    const deadline = Date.now() + LEASE;
    while (this.pending.length && Date.now() < deadline) await sleep(10);
    for (const s of this.others()) { this.heartbeat[s] = true; void this.replicate(s); }
    while ((this.match[to] ?? 0) < this.last && Date.now() < deadline && this.role === "leader") await sleep(10);
    let verdict: Verdict | "abandoned" = "abandoned";
    if (!abandon && this.role === "leader" && (this.match[to] ?? 0) >= this.last) {
      // Retried like any commit-path call: on a gray link one lost `TimeoutNow` otherwise abandons the whole transfer.
      // A repeat is harmless, because the target answers "leader" once it has won with this transfer id.
      verdict = await call(() => this.peer(to).timeoutNow(this.group, id), CALL_DEADLINE, COMMIT_RETRIES).catch(() => "lost" as Verdict);
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
  echo(s: string): number { return s.length; }

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
    reqs: Record<string, number>; reqsSeconds: number; learnerLat: number[]; maxPending: number }> {
    const out = { site: this.self, role: this.role, n: this.commitLat.length, entries: this.entrySizes.length, lat: [...this.commitLat],
      entryMs: [...this.entryMs], appendMs: { ...this.appendMs }, serveMs: { ...this.serveMs }, bytesOut: [...this.bytesOut], confirmSplit: this.confirmSplit.splice(0),
      timeline: [...this.timeline.entries()].map(([i, t]) => [i, ...t]), log: [...this.log],
      reqs: { ...this.reqs }, reqsSeconds: (Date.now() - this.reqsSince) / 1000, learnerLat: [...this.learnerLat], maxPending: this.maxPending };
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
