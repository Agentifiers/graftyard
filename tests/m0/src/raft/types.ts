// Shared by the M0-WP2 Raft members and edge. Constants are the design's (design/DESIGN.md, Shape, "Chains" and
// "The write path"). The state machine is the M0 register (src/tools.ts, `Op`), so histories check with porcupine/.

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
 *  `commitErrors`, or whose own storage confirm p99 passes `confirmP99Ms` with at least `confirmSlowMin` confirmations
 *  over it, transfers leadership away. 6, not 3: a healthy object's own storage confirmation passes 500 ms in bursts
 *  about 5 s apart, with no Raft work in the object, up to 4 times in 10 s (F35, `results/m0-wp4.md`). */
export const STEP_DOWN = { windowMs: 10_000, commitErrors: 0.05, confirmP99Ms: 500, confirmSlowMin: 6 };

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
export interface Ev { k: string; v: string; expect?: string | null }
/** `format` stands in for a record shape (M0-WP4): a member refuses an entry whose format is above its `READS`. */
export type Body =
  | { kind: "config"; voters: SiteKey[] }
  | { kind: "noop" }
  | { kind: "events"; events: Ev[]; requests: { rid: string; out: Out }[]; format?: number };

export interface Entry { term: number; index: number; prev: string; digest: string; at: number; body: Body }

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
export type RaftOp = Op | ModeOp;

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
