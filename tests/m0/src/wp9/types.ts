// M0-WP9 copy of src/raft/types.ts (the M0-WP2 prototype), with agent admission added. Constants are the design's
// (design/DESIGN.md, Shape, "Chains", "The write path", and "Agent load and agent safety"). The state machine is still
// the M0 register: a ref is a key, a push or merge is a compare-and-set on its ref's tip, and every other command is a put.

import type { Op, Out } from "../tools.ts";
export type { Op, Out };

export const LEASE = 9_000;
export const ELECTION_AFTER = 10_000;
export const DRIFT = 500;
export const CALL_DEADLINE = 1_000;
export const COMMIT_RETRIES = 2;
export const RESET_QUARANTINE = 300_000;
export const SUSPECT_COLOS = 2;
export const SUSPECT_WINDOW = 10_000;
/** design/DESIGN.md, Shape, `Health` and `STEP_DOWN`: over `windowMs`, a leader whose commit-path call error rate passes
 *  `commitErrors` or whose own storage confirm p99 passes `confirmP99Ms` transfers leadership away. */
/** Fourth pass: as `src/raft/types.ts` (design/DESIGN.md, `Health`), the confirm rule needs at least `confirmSlowMin`
 *  confirmations over `confirmP99Ms` in the window. */
/** Eighth pass, adopted: `commitFailMin`, the commit-error rule also needs at least 3 failed calls to every follower in
 *  the window (src/raft/member.ts has it behind `min3-`; here it is the default). */
export const STEP_DOWN = { windowMs: 10_000, commitErrors: 0.05, confirmP99Ms: 500, confirmSlowMin: 6, commitFailMin: 3 };

export type SiteKey = "a" | "b" | "c";
export const SITES: SiteKey[] = ["a", "b", "c"];
/** `Tri`: A enam, B weur, C apac. Site A is every group's `home`. */
export const HINT: Record<SiteKey, DurableObjectLocationHint> = { a: "enam", b: "weur", c: "apac" };
/** `Na3` (enam, wnam, weur), for groups whose name starts with "na3-", so one deploy measures both placements. */
const NA3: Record<SiteKey, DurableObjectLocationHint> = { a: "enam", b: "wnam", c: "weur" };
export const hintFor = (group: string, site: SiteKey) => (group.startsWith("na3-") ? NA3 : HINT)[site];
/** Script names for one deployment of the prototype: `graft-raft-*` (M0-WP2, B0-11's 48-hour run) or `graft-wp4-*`. */
export const scriptName = (prefix: string, k: SiteKey | "edge") => `${prefix}-${k}`;
export const memberName = (group: string, site: SiteKey) => `${group}#1#f${SITES.indexOf(site)}`;
/** A learner's object: kind `l` and its own slot, so it never shares an object with a voter (design/DESIGN.md, `Member`). */
export const learnerName = (group: string, slot: number) => `${group}#1#l${slot}`;
export const LEARNER_GAP = 50; // ms between a learner's read-index requests, one in flight (design/DESIGN.md, `fresh`)

/** One register write; `expect` is present for a compare-and-set, checked again by `verify` on followers. */
/** M0-WP9: `pack` is the byte length of this event's inline pack, whose bytes sit in `Entry.packs` in event order, and
 *  `p` is the path the push touched, which feed polls with `paths` match on. */
export interface Ev { k: string; v: string; expect?: string | null; pack?: number; p?: string }
/** `format` stands in for a record shape (M0-WP4): a member refuses an entry whose format is above its `READS`. */
export type Body =
  | { kind: "config"; voters: SiteKey[] }
  | { kind: "noop" }
  | { kind: "events"; events: Ev[]; requests: { rid: string; out: Out }[]; format?: number };

export interface Entry { term: number; index: number; prev: string; digest: string; at: number; body: Body; packs?: Uint8Array | null }

export interface AppendReq {
  term: number; leader: SiteKey; prevIndex: number; prevTerm: number; prevDigest: string; entries: Entry[]; commit: number;
  transfer: string | null; release?: number;
}
/** A refusal carries the refuser's release and the index it refused (design/DESIGN.md, `verify`). */
export interface AppendRes {
  term: number; ok: boolean; match: number; last: number; promise: boolean; refused?: string; refusedAt?: number; serveMs?: number;
  release?: number;
  /** The follower's `stepped_down_at`, so a leader's `STEP_DOWN` does not pick a target inside RESET_QUARANTINE. */
  steppedDownAt?: number | null;
}
export interface VoteReq {
  term: number; candidate: SiteKey; lastIndex: number; lastTerm: number; transfer: string | null;
  boots: [number, number]; steppedDownAt: number | null;
  /** M0-WP4: the candidate is drained, limited, or under `no_lead_until`, and campaigns only if no voter can lead. */
  cannotLead?: boolean;
}
export interface VoteRes { term: number; granted: boolean; why?: string }

/** A `SiteMode` entry for the prototype `Release` group (design/DESIGN.md, Shape, `SiteMode`). `by` names the key
 *  that signed it; signatures are not built in Milestone 0. `limit` and `unlimit` set and end the 1% lead limit. */
export type ModeOp = { kind: "mode"; key: string; site: SiteKey; mode: "drain" | "normal" | "limit" | "unlimit"; by: "drain" | "customer" };
/** M0-WP9: a push or merge carries its inline pack beside the compare-and-set on its ref. */
export type RaftOp = (Op & { pack?: Uint8Array; p?: string }) | ModeOp;

/** `decide` for `ModeOp` over the fold (`val`). The drain guard: a drain-key `Drain` is refused while another site is
 *  drained by either key. Each event carries the value it replaces as `expect`, so followers check it in `verify`. */
export function decideMode(op: ModeOp, val: (k: string) => string | null): { events: Ev[] } | { refused: string } {
  const ev = (k: string, v: string): Ev => ({ k, v, expect: val(k) });
  const drained = (s: SiteKey) => val("drainkey") === s || val(`cust:${s}`) === "drain";
  if (op.mode === "limit" || op.mode === "unlimit") return { events: [ev(`limit:${op.site}`, op.mode === "limit" ? "1" : "")] };
  if (op.by === "customer") return { events: [ev(`cust:${op.site}`, op.mode === "drain" ? "drain" : "")] };
  if (op.mode === "normal") {
    return val("drainkey") === op.site ? { events: [ev("drainkey", "")] } : { refused: `site ${op.site} is not drained by the drain key` };
  }
  const other = SITES.find((s) => s !== op.site && drained(s));
  if (other) return { refused: `drain guard: site ${other} is already drained` };
  return val("drainkey") === op.site ? { events: [] } : { events: [ev("drainkey", op.site)] };
}

export type OpRes = { out: Out; leader: SiteKey; commitMs?: number } | { notLeader: { term: number; leader: SiteKey | null } };
export type Verdict = "leader" | "promised" | "lost" | "busy";

export interface MemberStatus {
  site: SiteKey; role: "leader" | "follower" | "candidate"; term: number; votedFor: SiteKey | null;
  ledTerm: number | null; leader: SiteKey | null; last: number; commit: number; applied: number;
  promiseUntil: number; leaseEnd: number; boots: [number, number]; refusals: number; colo?: string; steppedDownAt?: number | null;
  // M0-WP4: this member's release, its refusal state, and the counters the site-day gates read.
  release?: number; noLeadUntil?: number | null; cannotFollow?: Partial<Record<SiteKey, number>>;
  applyErrors?: number; refusedByMe?: number; alarms?: string[]; stepDowns?: number; modes?: Modes;
}

// ---- M0-WP4: the prototype `Release` group and site modes ----

/** The prototype's `Release` group. It holds `SiteMode` as register keys and is exempt from drain itself. */
export const RELEASE_GROUP = "release";
/** design/DESIGN.md, "Install operations": isolates cache the `Release` view this long. */
export const POINTER_TTL = 5_000;
/** `drainkey` holds the one site drained by the drain-only key ("" for none), so the drain guard is one compare-and-set.
 *  `cust:<site>` is "drain" for a customer-key drain. `limit:<site>` is "1" while the site leads only 1% of its groups
 *  after an undrain under the 1% lead limit. */
export const MODE_KEYS = ["drainkey", ...SITES.map((s) => `cust:${s}`), ...SITES.map((s) => `limit:${s}`)];
export interface Modes { drained: SiteKey[]; limited: SiteKey[] }
export const NO_MODES: Modes = { drained: [], limited: [] };
export function modesFrom(v: Record<string, string | null>): Modes {
  const drained = SITES.filter((s) => v.drainkey === s || v[`cust:${s}`] === "drain");
  return { drained, limited: SITES.filter((s) => v[`limit:${s}`] === "1" && !drained.includes(s)) };
}
const hash = (s: string) => { let h = 0; for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0; return h; };
/** May a member on `site` lead `group`: not drained, and under the 1% lead limit only 1% of groups. */
export function mayLead(site: SiteKey, group: string, m: Modes): boolean {
  if (m.drained.includes(site)) return false;
  return !m.limited.includes(site) || hash(group) % 100 === 0;
}

// ---- M0-WP9: agent load and agent safety (design/DESIGN.md, Shape, "Agent load and agent safety") ----

export type AdmitClass = "Human" | "MergeQueue" | "Bot" | "Agent";
export const AGENT_SHARE = 0.5;
export const AGENT_FLOOR = 0.2;
export const DEMOTE_PER_MIN = 60;
export const AGENT_WAIT = 30_000;
export const AGENT_QUEUE_MAX = { cmds: 1_000, bytes: 64 << 20 };
/** 64 KiB since the third pass, as the design adopted (B0-32); a group named "ih256-..." gets the 256 KiB of the first two passes. */
export const INLINE_MAX_HOT = 64 << 10;
export const INLINE_MAX = 1 << 20;
/** Fifth pass, adopted: on a hot group an entry carries at most this many Agent-class inline pack bytes; an agent pack
 *  that would pass it is staged as `Stored` (refused at once while placement fails). Human and MergeQueue packs don't count. */
export const AGENT_INLINE_ENTRY = 128 << 10;
/** Sixth pass: how long the leader holds a commit call for its outcome before it answers the ticket instead (M0-WP9 guess). The edge waits COMMIT_HOLD + CALL_DEADLINE (2 s) for the reply, so a leader that answers nothing for 2 s, not 1 s, draws a `suspect`. */
export const COMMIT_HOLD = 1_000;
export const HOT_PUSHES = 2.0;
/** design/DESIGN.md, `EntryBody`: at most 1.5 MB including inline packs. */
export const ENTRY_MAX = 1_500_000;
/** design/AGENT-LAYER.md, Shape, "Activity feed". */
export const FEED_WAIT_MAX = 25_000;
export const FEED_BATCH = 1_000;
export const FEED_BATCH_ALL = 10_000;
export const FEED_RING = 10_000;
export const FEED_HELD_MAX = 1_000;

/** What a command is, for admission and for the run's counters. `push` and `merge` are compare-and-sets on a ref;
 *  `side` is every other write (PrSync, test-merge records, check runs, tasks, leases, REST writes). `stored` is the
 *  byte size of a pack the edge staged in buckets instead of inlining (`PackRef::Stored`; staging is not modeled). */
export interface Meta { cls: AdmitClass; actor: string; kind: "push" | "merge" | "side"; stored?: number }
/** The edge's view of the group, sent with every commit reply: the hot inline cap is in force, or agent admission is
 *  under pressure, so the edge stages packs above INLINE_MAX_HOT, or every pack, in buckets before it sends. */
export interface Hints { hot: boolean; pressure: boolean; /** `PackStore::place` succeeds; false while it fails (B0-31) */ placeOk: boolean }
/** Group-name variants, read anywhere in the name (M0-WP9; outside the design unless noted): "sdN-" needs N slow
 *  confirmations for STEP_DOWN's confirm rule, "ihN-" sets INLINE_MAX_HOT to N KiB, "nop-" makes every bucket's
 *  placement fail, "capN-" pins capacity at N commands/s, "hold-" turns STEP_DOWN off. */
export const variant = (group: string, tag: string): number | null => {
  const m = new RegExp(`(?:^|-)${tag}(\\d*)-`).exec(group);
  return m ? (m[1] ? Number(m[1]) : 0) : null;
};
export const refusalText = (kind: Meta["kind"], key: string, retry: number) =>
  kind === "push" ? `ng ${key} agent admission full, retry after ${retry}s` : `429 retry-after: ${retry} You have exceeded a secondary rate limit`;
export const REFUSAL_SHAPE = /^(ng \S+ agent admission full, retry after \d+s|429 retry-after: \d+ You have exceeded a secondary rate limit)$/;
