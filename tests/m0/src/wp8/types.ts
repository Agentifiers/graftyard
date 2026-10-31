// M0-WP8 (B0-28) restore-window prototype: a copy of the M0-WP2 Raft prototype (src/raft/) whose state machine is a
// git-ref fold instead of the register. Constants are the design's (design/DESIGN.md, Shape, "Chains", "Packs";
// design/SHARED.md, "Refs and pushes" and "Repository repair, tombstones, and retention"). Times named `at` are
// calendar times from the member's injected clock (`cal()` in member.ts), so a run replays 30 days in minutes; Raft's
// own timers (leases, deadlines) stay on the real clock.

export const LEASE = 9_000;
export const ELECTION_AFTER = 10_000;
export const DRIFT = 500;
export const CALL_DEADLINE = 1_000;
export const COMMIT_RETRIES = 2;
export const RESET_QUARANTINE = 300_000;
export const SUSPECT_COLOS = 2;
export const SUSPECT_WINDOW = 10_000;

export const DAY = 86_400_000;
/** design/DESIGN.md, Shape, Packs. */
export const RESTORE_WINDOW = 35 * DAY;
/** design/SHARED.md, "Refs and pushes": GC deletes an object 14 days after it leaves the root set. */
export const CRUFT_EXPIRY = 14 * DAY;
/** design/SHARED.md, Retention: chain entries stay in the live log 30 days, then archive. */
export const LOG_KEEP = 30 * DAY;
export const PACK_COMMIT_WINDOW = 3_600_000;
/** The orphan sweeper deletes an unmarked pack after 24 h, and REQUEST_RETENTION is 48 h (design/DESIGN.md). */
export const ORPHAN_AGE = DAY;
export const REQUEST_RETENTION = 2 * DAY;
/** The design's INLINE_MAX is 1 MiB. The replay's packs are a few KB, so the prototype scales it to 8 KiB to get both
 *  inline and stored packs [scaled]. */
export const INLINE_MAX = 8 << 10;
/** GC deletes "at a capped rate": at most this many packs per GC run [guess]. */
export const GC_RATE = 20;
export const ARCHIVE_MAX_ENTRIES = 1_000;
/** The design's LOG_MAX_BYTES is 2 GB per member, scaled to 160 KiB so the 30-day rule does most archiving and the byte
 *  rule only caps the log [scaled]. A first run at 24 KiB showed that `Archived` and `Promoted` entries themselves add
 *  log bytes, so a cap near the log's size re-arms itself and archives almost the whole log. */
export const LOG_MAX_BYTES = 160 << 10;
/** The design's SNAPSHOT_EVERY is 100,000 entries; scaled to 40. "At least once on each day the group commits" is kept. */
export const SNAPSHOT_EVERY = 40;

export type SiteKey = "a" | "b" | "c";
export const SITES: SiteKey[] = ["a", "b", "c"];
/** `Tri`: A enam, B weur, C apac, for members and for each site's bucket. */
export const HINT: Record<SiteKey, DurableObjectLocationHint> = { a: "enam", b: "weur", c: "apac" };
export const PREFIX = "graft-wp8";
export const scriptName = (k: SiteKey | "edge") => `${PREFIX}-${k}`;
export const bucketName = (k: SiteKey) => `${PREFIX}-${k}`;
export const memberName = (group: string, site: SiteKey) => `${group}#1#f${SITES.indexOf(site)}`;
/** The next site in config order: an archive or snapshot goes to the writer's own site's bucket and this one. */
export const nextSite = (s: SiteKey): SiteKey => SITES[(SITES.indexOf(s) + 1) % 3];

// ---- git objects and packs ----

export type ObjType = "commit" | "tree" | "blob" | "tag";
/** A git object with its real SHA-1 `oid`; `data` is the object body, base64. `links` are the oids it names (a commit's
 *  tree and parents, a tree's entries except gitlinks, a tag's object), parsed by the edge from `data`. */
export interface GitObj { oid: string; type: ObjType; data: string; links: string[] }
/** design/DESIGN.md, `PackRef`. `sites` are the buckets the create-only puts reached (at least two). */
export interface StoredPack { id: string; checksum: string; bytes: number; sites: SiteKey[]; gc?: "geo" | "cruft" }
export type PackRef = { inline: GitObj[] } | { stored: StoredPack };

export type ArchiveKind = "archive" | "snapshot";
/** design/DESIGN.md, `Archived { key, from, to, digest }`. `fromAt` and `toAt` (the `at` of entries `from` and `to`)
 *  and `sites` are this prototype's additions: `RestoredTo` needs the last index at or before a time that may lie in an
 *  archived range, and a reader needs to know which buckets to read. */
export interface ArchivedRec { kind: ArchiveKind; key: string; from: number; to: number; digest: string; fromAt: number; toAt: number; sites: SiteKey[] }

export interface RefUpdate { ref: string; old: string | null; new: string | null }

/** Fold events. `ref` is the `Refs`-shaped difference, with the value it replaces checked again by `verify`. */
export type Ev =
  | { t: "ref"; ref: string; old: string | null; new: string | null }
  | { t: "pack"; pack: PackRef }
  | { t: "restored"; to: number; toAt: number; refs: string[]; targets: Record<string, string | null>; readAt: number }
  | { t: "archived"; rec: ArchivedRec }
  | { t: "promoted"; from: number; to: number; pack: StoredPack }
  /** design/SHARED.md, `NetworkEvent::Compacted { consumed, produced }`: GC's geometric and cruft packs. */
  | { t: "compacted"; consumed: string[]; produced: StoredPack[]; readAt: number };

/** What the leader read before `decide`, as `deps` are read once through `fresh`: `to` and its `at` (from the live log
 *  or an archive in R2), the targets it found in `ref_log`, and the reachability walk from them over the live and cruft
 *  packs. `decide` recomputes the targets from the fold, and refuses unless they match and nothing is missing. */
export interface RestoreCheck {
  toAt: number; targets: Record<string, string | null>; objects: number; missing: string[]; unreadable: string[]; via: string;
  /** The applied index the walk read the pack set at; `decide` refuses if a `Compacted` committed after it. */
  readAt: number;
}

export type Op =
  /** `readAt` is the pack-set index the connectivity check read; set only when the push names objects it does not carry. */
  | { kind: "push"; updates: RefUpdate[]; pack: PackRef | null; readAt?: number }
  | { kind: "archived"; rec: ArchivedRec }
  | { kind: "promoted"; from: number; to: number; pack: StoredPack }
  /** `readAt` is the applied index GC read its roots at. `decide` refuses if a ref changed after it, because a push
   *  could have pointed a ref at an object this GC dropped as expired cruft. */
  | { kind: "compacted"; consumed: string[]; produced: StoredPack[]; readAt: number }
  | { kind: "restore"; to: number; refs: string[]; check: RestoreCheck };
export interface Out { ok: boolean; why?: string; value?: unknown }

export type Body =
  | { kind: "config"; voters: SiteKey[] }
  | { kind: "noop" }
  | { kind: "events"; events: Ev[]; requests: { rid: string; out: Out }[] };

export interface Entry { term: number; index: number; prev: string; digest: string; at: number; body: Body }

export interface AppendReq {
  term: number; leader: SiteKey; prevIndex: number; prevTerm: number; prevDigest: string; entries: Entry[]; commit: number;
  transfer: string | null;
}
export interface AppendRes { term: number; ok: boolean; match: number; last: number; refused?: string; installing?: boolean }
export interface VoteReq { term: number; candidate: SiteKey; lastIndex: number; lastTerm: number; transfer: string | null }
export interface VoteRes { term: number; granted: boolean; why?: string }
/** design/DESIGN.md, `InstallSnapshot { term, core, metadata, archives, through }`; this fold has one tier. */
export interface InstallReq { term: number; leader: SiteKey; core: ArchivedRec | null /* genesis */; archives: ArchivedRec[]; through: number }

export type OpRes = { out: Out; entry: number } | { notLeader: { term: number; leader: SiteKey | null } };
export type Verdict = "leader" | "promised" | "lost" | "busy";

/** Test-only switches, set per member through the edge. `offline` makes every RPC to it fail. `skipSecondPut` makes
 *  this member's archive and snapshot uploads reach only its own bucket, as a failed second put. `abandonAfterPut`
 *  stops a leader's archive step after both puts, as a leader deposed before it commits `Archived`. `commitWithoutHead`
 *  makes a leader commit `Archived` without HEADing both buckets (a defective leader). `noRepair` stops this member
 *  re-putting a committed archive that is missing from one of its buckets. `blindTruncate` makes it truncate by age
 *  alone, with no `Archived` check, so the edge's audit can show it catches a bad truncation. */
export interface Flags {
  offline?: boolean; skipSecondPut?: boolean; abandonAfterPut?: boolean; commitWithoutHead?: boolean; noRepair?: boolean; blindTruncate?: boolean;
}

/** Variants by group-name prefix, so the rule is the same on every member of a group: any other name is the design as
 *  written. `noreflog-` drops `ref_log` from GC roots (control). `bywrite-` prunes `ref_log` by when a value was
 *  written instead of when it was replaced (the other reading of "every ref value of the last RESTORE_WINDOW"). */
export type Variant = "design" | "noreflog" | "bywrite";
export const variantOf = (group: string): Variant => (group.startsWith("noreflog-") ? "noreflog" : group.startsWith("bywrite-") ? "bywrite" : "design");
/** Each member's clock skew on top of the test's calendar, so `at` must stay monotonic across a leader change. */
export const SKEW: Record<SiteKey, number> = { a: 0, b: 120_000, c: -120_000 };

export interface MemberStatus {
  site: SiteKey; role: "leader" | "follower" | "candidate"; term: number; leader: SiteKey | null; last: number; commit: number;
  applied: number; base: number; cal: number; flags: Flags; foldDigest: string; logBytes: number; archThrough: number;
  snapThrough: number; refusedTruncations: number; installs: number; notes: string[];
}
/** One member's truncations and installs, as it recorded them: the base before and after, and the keys it relied on. */
export interface TruncRow { real: number; cal: number; kind: "truncate" | "install" | "blind"; before: number; after: number; keys: string[] }

/** G6 race switches, by tokens in the group name so every member agrees: `-nocg-` turns off the `Compacted` read-index
 *  guard, `-nopg-` the push's, `-norg-` the `RestoredTo`'s. `-coarse-` makes GC replan on every refusal instead of
 *  resubmitting when nothing it expires became reachable. */
export const guardsOf = (group: string) => ({ compacted: !group.includes("-nocg-"), push: !group.includes("-nopg-"),
  restore: !group.includes("-norg-"), cheapRerun: !group.includes("-coarse-") });
/** One GC run under the race harness. */
export interface GcRace {
  readAt: number[]; committed: boolean; attempts: number; cheapReruns: number; fullReruns: number; expired: number; savedByRerun: number;
  refusals: string[]; ms: number; consumed: number; orphanPacks: number; entry: number;
}
