// M0-WP8 (B0-28) member: a trimmed copy of the M0-WP2 Raft member (src/raft/member.ts) whose state machine is a git-ref
// fold, deployed as graft-wp8-a (enam), graft-wp8-b (weur), and graft-wp8-c (apac), each binding the other two sites'
// class with `script_name` and all three site buckets graft-wp8-a/b/c.
//
// Kept from the copy: terms, votes, the log with chained digests, AppendEntries with `verify`, majority commit, one entry
// in flight, and request-id dedupe. Dropped, because B0-28 does not measure them: leases, pre-vote, `suspect`, transfer,
// learners, and site modes. Elections run only when the test asks (`campaign`).
//
// Added (design/DESIGN.md, Shape, "Packs"; design/SHARED.md, "Refs and pushes" and "Repository repair, tombstones,
// and retention"): fold tables `refs` and `ref_log` (every ref value until RESTORE_WINDOW after it was replaced),
// stored and inline packs, `Promoted`, archives and snapshots in two buckets with `Archived` committed after the leader
// HEADs both, truncation only through committed ranges this member HEADs in two buckets, `InstallSnapshot`, GC with
// cruft packs and 14-day expiry, and `RestoredTo`. Calendar time comes from `cal()`, which the test sets.

import { DurableObject } from "cloudflare:workers";
import { sha256hex } from "./sha256.ts";
import {
  archiveText, bucket, closure, digestOf, getVerified, headIn, loadPacks, markCommitted, objectOk, packId, packTime, placePack,
  putOnce, R2, sha256, ZERO, type Buckets, type PackView,
} from "./store.ts";
import {
  CRUFT_EXPIRY, DAY, GC_RATE, LOG_KEEP, LOG_MAX_BYTES, ARCHIVE_MAX_ENTRIES, PACK_COMMIT_WINDOW, RESTORE_WINDOW, SITES, SKEW, SNAPSHOT_EVERY,
  memberName, HINT, nextSite, variantOf, guardsOf,
  type AppendReq, type AppendRes, type ArchivedRec, type Body, type Entry, type Ev, type Flags, type GitObj, type InstallReq,
  type GcRace, type MemberStatus, type RestoreCheck, type Op, type Out, type RefUpdate, type SiteKey, type StoredPack, type TruncRow, type VoteReq, type VoteRes,
} from "./types.ts";

export interface MemberEnv extends Buckets {
  SITE: SiteKey;
  MEMBER_A: DurableObjectNamespace<Wp8Member>;
  MEMBER_B: DurableObjectNamespace<Wp8Member>;
  MEMBER_C: DurableObjectNamespace<Wp8Member>;
}
export type NotLeader = { notLeader: { term: number; leader: SiteKey | null } };
export type SubmitRes = { out: Out; entry: number } | NotLeader;

const CALL_DEADLINE = 3_000;
const INSTALL_DEADLINE = 30_000;
const CATCHUP_MAX = 100;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
async function call<T>(fn: () => Promise<T>, ms = CALL_DEADLINE): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try { return await Promise.race([fn(), new Promise<never>((_, rej) => { timer = setTimeout(() => rej(new Error("call deadline")), ms); })]); }
  finally { if (timer !== undefined) clearTimeout(timer); }
}

/** The fold's tables, in snapshot and digest order, with their primary keys. `req` dedupes request ids. */
const FOLD: [string, string][] = [["refs", "ref"], ["ref_log", "ref, set_idx"], ["packs", "id"], ["inline_objs", "oid"],
  ["archived", "key"], ["fmeta", "k"], ["req", "rid"]];

type Row = Record<string, SqlStorageValue>;

export class Wp8Member extends DurableObject<MemberEnv> {
  private sql = this.ctx.storage.sql;
  private self: SiteKey;
  private group = "";
  // Persisted in `state`.
  private term = 0;
  private votedFor: SiteKey | null = null;
  private commitIdx = 0;
  private applied = 0;
  private base = 0; // entries 1..base are truncated; `base_*` describe entry `base`
  private baseTerm = 0;
  private baseDigest = ZERO;
  private baseAt = 0;
  private offset = 0; // calendar ms minus real ms, set by the test
  private flags: Flags = {};
  private lastGcDay = -1;
  // Volatile.
  private role: "leader" | "follower" | "candidate" = "follower";
  private leader: SiteKey | null = null;
  private last = 0;
  private lastTerm = 0;
  private lastDigest = ZERO;
  private lastAt = 0;
  private next: Partial<Record<SiteKey, number>> = {};
  private match: Partial<Record<SiteKey, number>> = {};
  private sentCommit: Partial<Record<SiteKey, number>> = {};
  private inflight: Partial<Record<SiteKey, Promise<void>>> = {};
  private chain: Promise<unknown> = Promise.resolve();
  private installing = false;
  private refusedTruncations = 0;
  private installs = 0;
  private notes: string[] = [];
  private note(m: string): void { this.notes.push(`${new Date(this.cal()).toISOString().slice(0, 16)} ${m}`); if (this.notes.length > 200) this.notes.shift(); }

  constructor(ctx: DurableObjectState, env: MemberEnv) {
    super(ctx, env);
    this.self = env.SITE;
    const sql = this.sql;
    sql.exec(`CREATE TABLE IF NOT EXISTS state (id INTEGER PRIMARY KEY CHECK (id = 1), grp TEXT, term INTEGER, voted_for TEXT,
      commit_idx INTEGER, applied INTEGER, base INTEGER, base_term INTEGER, base_digest TEXT, base_at INTEGER, off INTEGER,
      flags TEXT, last_gc_day INTEGER)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS log (idx INTEGER PRIMARY KEY, term INTEGER, prev TEXT, digest TEXT, at INTEGER, body TEXT, bytes INTEGER)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS refs (ref TEXT PRIMARY KEY, oid TEXT)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS ref_log (ref TEXT, oid TEXT, set_idx INTEGER, set_at INTEGER, replaced_idx INTEGER,
      replaced_at INTEGER, PRIMARY KEY (ref, set_idx))`);
    sql.exec(`CREATE TABLE IF NOT EXISTS packs (id TEXT PRIMARY KEY, json TEXT, idx INTEGER)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS inline_objs (oid TEXT PRIMARY KEY, json TEXT, idx INTEGER)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS archived (key TEXT PRIMARY KEY, kind TEXT, frm INTEGER, too INTEGER, digest TEXT,
      from_at INTEGER, to_at INTEGER, sites TEXT, idx INTEGER)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS fmeta (k TEXT PRIMARY KEY, v INTEGER)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS req (rid TEXT PRIMARY KEY, idx INTEGER, out TEXT)`);
    // Member-local, not fold: this member's truncations and installs, and GC's pending pack deletions.
    sql.exec(`CREATE TABLE IF NOT EXISTS trunc (real INTEGER, cal INTEGER, kind TEXT, before INTEGER, after INTEGER, keys TEXT)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS gc_delete (id TEXT PRIMARY KEY)`);
    sql.exec(`CREATE TABLE IF NOT EXISTS lmeta (k TEXT PRIMARY KEY, v INTEGER)`);
    const r = sql.exec<Row>(`SELECT * FROM state WHERE id = 1`).toArray()[0];
    if (r) {
      this.group = r.grp as string; this.term = r.term as number; this.votedFor = r.voted_for as SiteKey | null;
      this.commitIdx = r.commit_idx as number; this.applied = r.applied as number; this.base = r.base as number;
      this.baseTerm = r.base_term as number; this.baseDigest = r.base_digest as string; this.baseAt = r.base_at as number;
      this.offset = r.off as number; this.flags = JSON.parse(r.flags as string); this.lastGcDay = r.last_gc_day as number;
    }
    this.loadTail();
  }

  // ---- persistence and the clock ----

  private save(): void {
    this.sql.exec(`INSERT OR REPLACE INTO state VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`, this.group, this.term, this.votedFor,
      this.commitIdx, this.applied, this.base, this.baseTerm, this.baseDigest, this.baseAt, this.offset, JSON.stringify(this.flags), this.lastGcDay);
  }
  private loadTail(): void {
    const t = this.sql.exec<Row>(`SELECT idx, term, digest, at FROM log ORDER BY idx DESC LIMIT 1`).toArray()[0];
    if (t) { this.last = t.idx as number; this.lastTerm = t.term as number; this.lastDigest = t.digest as string; this.lastAt = t.at as number; }
    else { this.last = this.base; this.lastTerm = this.baseTerm; this.lastDigest = this.baseDigest; this.lastAt = this.baseAt; }
  }
  /** Calendar time: real time plus the test's offset plus this site's skew. */
  private cal(): number { return Date.now() + this.offset + SKEW[this.self]; }
  private setGroup(group: string): void { if (!this.group) { this.group = group; this.save(); } }
  private online(): void { if (this.flags.offline) throw new Error(`m0 injected fault: ${this.self} offline`); }
  private peer(s: SiteKey): DurableObjectStub<Wp8Member> {
    const ns = this.env[`MEMBER_${s.toUpperCase()}` as "MEMBER_A"];
    return ns.get(ns.idFromName(memberName(this.group, s)), { locationHint: HINT[s] });
  }
  private others(): SiteKey[] { return SITES.filter((s) => s !== this.self); }
  private entry(i: number): Entry | undefined {
    const r = this.sql.exec<Row>(`SELECT * FROM log WHERE idx = ?`, i).toArray()[0];
    return r && { index: r.idx as number, term: r.term as number, prev: r.prev as string, digest: r.digest as string, at: r.at as number,
      body: JSON.parse(r.body as string) };
  }
  private entries(from: number, to: number): Entry[] {
    return this.sql.exec<Row>(`SELECT * FROM log WHERE idx BETWEEN ? AND ? ORDER BY idx`, from, to).toArray().map((r) => ({
      index: r.idx as number, term: r.term as number, prev: r.prev as string, digest: r.digest as string, at: r.at as number,
      body: JSON.parse(r.body as string) }));
  }
  private storeEntry(e: Entry): void {
    const body = JSON.stringify(e.body);
    this.sql.exec(`INSERT OR REPLACE INTO log VALUES (?, ?, ?, ?, ?, ?, ?)`, e.index, e.term, e.prev, e.digest, e.at, body, body.length + 200);
    this.last = e.index; this.lastTerm = e.term; this.lastDigest = e.digest; this.lastAt = e.at;
  }
  private truncateFrom(i: number): void {
    if (i <= this.commitIdx) throw new Error(`refusing to drop committed index ${i}`);
    this.sql.exec(`DELETE FROM log WHERE idx >= ?`, i);
    this.loadTail();
  }

  // ---- fold reads ----

  private meta(k: string): number { return (this.sql.exec<Row>(`SELECT v FROM fmeta WHERE k = ?`, k).toArray()[0]?.v as number) ?? 0; }
  private setMeta(k: string, v: number): void { this.sql.exec(`INSERT OR REPLACE INTO fmeta VALUES (?, ?)`, k, v); }
  /** Member-local counters, outside the fold. */
  private bump(k: string, by: number): void { this.sql.exec(`INSERT INTO lmeta VALUES (?, ?) ON CONFLICT (k) DO UPDATE SET v = v + ?`, k, by, by); }
  private refGet(ref: string): string | null { return (this.sql.exec<Row>(`SELECT oid FROM refs WHERE ref = ?`, ref).toArray()[0]?.oid as string) ?? null; }
  /** Each ref's value at index `to`, from `ref_log`; a ref with no row covering `to` was absent then. */
  private refLogAt(to: number, refs: string[]): Record<string, string | null> {
    const out: Record<string, string | null> = {};
    for (const ref of refs) {
      out[ref] = (this.sql.exec<Row>(`SELECT oid FROM ref_log WHERE ref = ? AND set_idx <= ? AND (replaced_idx IS NULL OR replaced_idx > ?)
        ORDER BY set_idx DESC LIMIT 1`, ref, to, to).toArray()[0]?.oid as string) ?? null;
    }
    return out;
  }
  private records(kind: "archive" | "snapshot"): ArchivedRec[] {
    return this.sql.exec<Row>(`SELECT * FROM archived WHERE kind = ? ORDER BY frm, too`, kind).toArray().map((r) => ({
      kind, key: r.key as string, from: r.frm as number, to: r.too as number, digest: r.digest as string, fromAt: r.from_at as number,
      toAt: r.to_at as number, sites: JSON.parse(r.sites as string) }));
  }
  /** Committed archive records that cover 1..n with no gap, in order. */
  private archChain(): ArchivedRec[] {
    const out: ArchivedRec[] = [];
    let t = 0;
    for (const r of this.records("archive")) if (r.from === t + 1) { out.push(r); t = r.to; }
    return out;
  }
  private archThrough(): number { return this.archChain().at(-1)?.to ?? 0; }
  private packView(): PackView & { at: number } {
    return { at: this.applied,
      stored: this.sql.exec<Row>(`SELECT json FROM packs ORDER BY id`).toArray().map((r) => JSON.parse(r.json as string) as StoredPack),
      inline: this.sql.exec<Row>(`SELECT json FROM inline_objs ORDER BY oid`).toArray().map((r) => JSON.parse(r.json as string) as GitObj) };
  }
  private foldRows(): Record<string, Row[]> {
    return Object.fromEntries(FOLD.map(([t, pk]) => [t, this.sql.exec<Row>(`SELECT * FROM ${t} ORDER BY ${pk}`).toArray()]));
  }

  // ---- apply ----

  private setRef(ref: string, oid: string | null, idx: number, at: number): void {
    if (this.refGet(ref) === oid) return;
    this.sql.exec(`UPDATE ref_log SET replaced_idx = ?, replaced_at = ? WHERE ref = ? AND replaced_idx IS NULL`, idx, at, ref);
    this.sql.exec(`INSERT INTO ref_log VALUES (?, ?, ?, ?, NULL, NULL)`, ref, oid, idx, at);
    if (oid === null) this.sql.exec(`DELETE FROM refs WHERE ref = ?`, ref);
    else this.sql.exec(`INSERT OR REPLACE INTO refs VALUES (?, ?)`, ref, oid);
    this.setMeta("last_ref_change", idx);
  }
  private applyEv(ev: Ev, e: Entry): void {
    switch (ev.t) {
      case "ref": this.setRef(ev.ref, ev.new, e.index, e.at); break;
      case "restored": for (const ref of ev.refs) this.setRef(ref, ev.targets[ref], e.index, e.at); break;
      case "pack":
        if ("inline" in ev.pack) for (const o of ev.pack.inline) this.sql.exec(`INSERT OR IGNORE INTO inline_objs VALUES (?, ?, ?)`, o.oid, JSON.stringify(o), e.index);
        else this.sql.exec(`INSERT OR REPLACE INTO packs VALUES (?, ?, ?)`, ev.pack.stored.id, JSON.stringify(ev.pack.stored), e.index);
        break;
      case "archived": {
        const r = ev.rec;
        this.sql.exec(`INSERT OR REPLACE INTO archived VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`, r.key, r.kind, r.from, r.to, r.digest, r.fromAt, r.toAt,
          JSON.stringify(r.sites), e.index);
        break;
      }
      case "promoted":
        this.sql.exec(`INSERT OR REPLACE INTO packs VALUES (?, ?, ?)`, ev.pack.id, JSON.stringify(ev.pack), e.index);
        this.sql.exec(`DELETE FROM inline_objs WHERE idx <= ?`, ev.to);
        break;
      case "compacted":
        for (const id of ev.consumed) this.sql.exec(`DELETE FROM packs WHERE id = ?`, id);
        for (const p of ev.produced) this.sql.exec(`INSERT OR REPLACE INTO packs VALUES (?, ?, ?)`, p.id, JSON.stringify(p), e.index);
        this.setMeta("last_compacted", e.index);
        break;
    }
  }
  /** Applies one committed entry; `ref_log` drops values replaced more than RESTORE_WINDOW before the entry's `at`. */
  private apply(e: Entry): void {
    if (e.body.kind === "events") {
      for (const ev of e.body.events) this.applyEv(ev, e);
      for (const r of e.body.requests) this.sql.exec(`INSERT OR IGNORE INTO req VALUES (?, ?, ?)`, r.rid, e.index, JSON.stringify(r.out));
      // A pruned row's `replaced_idx` is kept per ref in `fmeta`, so `decide` can tell a lost value from an absent ref.
      const where = variantOf(this.group) === "bywrite" ? "replaced_idx IS NOT NULL AND set_at < ?" : "replaced_idx IS NOT NULL AND replaced_at < ?";
      for (const r of this.sql.exec<Row>(`SELECT ref, MAX(replaced_idx) AS m FROM ref_log WHERE ${where} GROUP BY ref`, e.at - RESTORE_WINDOW).toArray())
        this.setMeta(`pruned:${r.ref}`, Math.max(this.meta(`pruned:${r.ref}`), r.m as number));
      this.sql.exec(`DELETE FROM ref_log WHERE ${where}`, e.at - RESTORE_WINDOW);
    }
    this.applied = e.index;
  }
  private applyUpTo(i: number): void {
    while (this.applied < Math.min(i, this.last)) this.apply(this.entry(this.applied + 1)!);
  }

  // ---- decide: pure over the fold and what the leader read ----

  private decide(op: Op, at: number): { events: Ev[]; out: Out } | { refused: string } {
    switch (op.kind) {
      case "push": {
        const events: Ev[] = [];
        if (op.pack && "stored" in op.pack && packTime(op.pack.stored.id) < at - PACK_COMMIT_WINDOW) return { refused: "pack older than PACK_COMMIT_WINDOW" };
        if (op.readAt !== undefined && guardsOf(this.group).push && this.meta("last_compacted") > op.readAt)
          return { refused: "packs compacted since the connectivity check" };
        if (op.pack) events.push({ t: "pack", pack: op.pack });
        for (const u of op.updates) {
          if (this.refGet(u.ref) !== u.old) return { refused: `stale ${u.ref}` };
          events.push({ t: "ref", ref: u.ref, old: u.old, new: u.new });
        }
        return { events, out: { ok: true } };
      }
      case "archived": {
        const r = op.rec;
        if (this.sql.exec(`SELECT 1 FROM archived WHERE key = ?`, r.key).toArray().length) return { refused: "already archived" };
        if (r.sites.length < 2 || new Set(r.sites).size < 2) return { refused: "archive needs two buckets" };
        if (r.kind === "archive") {
          if (r.from !== this.archThrough() + 1) return { refused: `archive must start at ${this.archThrough() + 1}` };
          const unpromoted = this.sql.exec(`SELECT 1 FROM inline_objs WHERE idx <= ? LIMIT 1`, r.to).toArray().length;
          if (unpromoted) return { refused: "range has inline packs not yet Promoted" };
        }
        return { events: [{ t: "archived", rec: r }], out: { ok: true } };
      }
      case "promoted": return { events: [{ t: "promoted", from: op.from, to: op.to, pack: op.pack }], out: { ok: true } };
      case "compacted": {
        if (guardsOf(this.group).compacted && this.meta("last_ref_change") > op.readAt) return { refused: "refs changed since GC read its roots" };
        for (const id of op.consumed) if (!this.sql.exec(`SELECT 1 FROM packs WHERE id = ?`, id).toArray().length) return { refused: `pack ${id} not live` };
        return { events: [{ t: "compacted", consumed: op.consumed, produced: op.produced, readAt: op.readAt }], out: { ok: true } };
      }
      case "restore": {
        const c = op.check;
        if (op.to > this.commitIdx) return { refused: "to is not committed" };
        if (c.toAt < at - RESTORE_WINDOW) return { refused: "to is outside RESTORE_WINDOW" };
        for (const ref of op.refs) if (this.meta(`pruned:${ref}`) > op.to) return { refused: `ref_log no longer holds ${ref} at ${op.to}` };
        if (guardsOf(this.group).restore && this.meta("last_compacted") > c.readAt) return { refused: "packs compacted since the reachability check" };
        const targets = this.refLogAt(op.to, op.refs);
        if (JSON.stringify(targets) !== JSON.stringify(c.targets)) return { refused: "targets differ from ref_log" };
        if (c.missing.length || c.unreadable.length) return { refused: `unreachable: ${c.missing.length} objects missing, ${c.unreadable.length} packs unreadable` };
        return { events: [{ t: "restored", to: op.to, toAt: c.toAt, refs: op.refs, targets, readAt: c.readAt }], out: { ok: true, value: targets } };
      }
    }
  }

  // ---- leader: submit, one entry in flight ----

  private submitting: Promise<unknown> = Promise.resolve();
  async submit(group: string, rid: string, op: Op): Promise<SubmitRes> {
    this.setGroup(group); this.online();
    const run = this.submitting.then(() => this.submitOne(rid, op));
    this.submitting = run.catch(() => {});
    return run;
  }
  private async submitOne(rid: string, op: Op): Promise<SubmitRes> {
    if (this.role !== "leader") return { notLeader: { term: this.term, leader: this.leader } };
    const prior = this.sql.exec<Row>(`SELECT idx, out FROM req WHERE rid = ?`, rid).toArray()[0];
    if (prior) return { out: JSON.parse(prior.out as string), entry: prior.idx as number };
    this.applyUpTo(this.commitIdx);
    const at = Math.max(this.cal(), this.lastAt); // `at` never decreases (design/DESIGN.md, `Lease`)
    const d = this.decide(op, at);
    if ("refused" in d) { this.note(`refused ${op.kind}: ${d.refused}`); return { out: { ok: false, why: d.refused }, entry: 0 }; }
    const index = await this.appendAsLeader({ kind: "events", events: d.events, requests: [{ rid, out: d.out }] }, at);
    return { out: d.out, entry: index };
  }
  private async appendAsLeader(body: Body, at: number): Promise<number> {
    const e0 = { term: this.term, index: this.last + 1, prev: this.lastDigest, at, body };
    const e: Entry = { ...e0, digest: digestOf(e0) };
    this.storeEntry(e); this.save();
    const end = Date.now() + 15_000;
    for (const s of this.others()) void this.replicate(s);
    while (this.commitIdx < e.index) {
      if (this.role !== "leader" || Date.now() > end) throw new Error(`entry ${e.index} not committed`);
      await sleep(20);
      if (this.commitIdx < e.index) for (const s of this.others()) void this.replicate(s);
    }
    this.applyUpTo(this.commitIdx); this.save();
    if (body.kind === "events") for (const ev of body.events) {
      const p = ev.t === "pack" && "stored" in ev.pack ? ev.pack.stored : ev.t === "promoted" ? ev.pack : null;
      if (p) await markCommitted(this.env, this.group, p, e.index).catch(() => {});
      if (ev.t === "compacted") {
        for (const q of ev.produced) await markCommitted(this.env, this.group, q, e.index).catch(() => {});
        for (const id of ev.consumed) this.sql.exec(`INSERT OR IGNORE INTO gc_delete VALUES (?)`, id);
      }
    }
    return e.index;
  }
  private replicate(s: SiteKey): Promise<void> {
    return (this.inflight[s] ??= this.replicateLoop(s).finally(() => { delete this.inflight[s]; }));
  }
  private async replicateLoop(s: SiteKey): Promise<void> {
    const term = this.term;
    let failures = 0;
    while (this.role === "leader" && this.term === term && failures < 4) {
      const from = this.next[s] ?? this.last + 1;
      if (from > this.last && (this.sentCommit[s] ?? 0) >= this.commitIdx) return;
      if (from <= this.base) {
        // The follower needs entries this leader has truncated: InstallSnapshot.
        const req = this.installReq();
        try {
          const r = await call(() => this.peer(s).installSnapshot(this.group, req), INSTALL_DEADLINE);
          if (r.term > this.term) { this.stepDown(r.term); return; }
          if (r.ok) { this.match[s] = r.match; this.next[s] = r.match + 1; } else { failures++; await sleep(500); }
        } catch (e) { failures++; this.note(`install to ${s} failed: ${String(e).slice(0, 80)}`); await sleep(500); }
        continue;
      }
      const prev = from - 1;
      const pe = prev === this.base ? null : this.entry(prev);
      const req: AppendReq = { term, leader: this.self, prevIndex: prev, prevTerm: pe ? pe.term : this.baseTerm,
        prevDigest: pe ? pe.digest : this.baseDigest, entries: this.entries(from, Math.min(this.last, from + CATCHUP_MAX - 1)),
        commit: this.commitIdx, transfer: null };
      let res: AppendRes;
      try { res = await call(() => this.peer(s).append(this.group, req)); failures = 0; }
      catch { failures++; await sleep(300); continue; }
      if (res.term > this.term) { this.stepDown(res.term); return; }
      if (res.installing) { await sleep(500); continue; }
      if (res.ok) {
        this.match[s] = Math.max(this.match[s] ?? 0, res.match); this.next[s] = res.match + 1;
        this.sentCommit[s] = req.commit;
        const best = Math.max(...this.others().map((x) => this.match[x] ?? 0));
        if (best > this.commitIdx && this.entry(best)?.term === this.term) this.commitIdx = best;
      } else if (res.refused) { this.note(`${s} refused: ${res.refused}`); return; }
      else this.next[s] = Math.max(1, Math.min(from - 1, res.last + 1));
    }
  }
  private stepDown(term: number): void {
    if (term > this.term) { this.term = term; this.votedFor = null; }
    this.role = "follower"; this.save();
  }

  // ---- RPC: member to member ----

  async append(group: string, req: AppendReq): Promise<AppendRes> {
    this.setGroup(group); this.online();
    const no = (extra: Partial<AppendRes> = {}): AppendRes => ({ term: this.term, ok: false, match: 0, last: this.last, ...extra });
    if (req.term < this.term) return no();
    if (req.term > this.term || this.role !== "follower") this.stepDown(req.term);
    this.leader = req.leader;
    if (this.installing) return no({ installing: true });
    if (req.prevIndex > this.last) return no();
    if (req.prevIndex >= this.base) {
      const have = req.prevIndex === this.base ? this.baseDigest : this.entry(req.prevIndex)?.digest;
      if (have !== req.prevDigest) {
        if (req.prevIndex > this.commitIdx) this.truncateFrom(req.prevIndex);
        return no({ last: Math.min(this.last, req.prevIndex - 1) });
      }
    }
    for (const e of req.entries) {
      if (e.index <= this.base) continue;
      if (e.index <= this.last) { if (this.entry(e.index)?.digest === e.digest) continue; this.truncateFrom(e.index); }
      this.applyUpTo(Math.min(req.commit, e.index - 1));
      const why = this.verify(e);
      if (why) { this.save(); return no({ refused: why }); }
      this.storeEntry(e);
    }
    this.commitIdx = Math.max(this.commitIdx, Math.min(req.commit, req.prevIndex + req.entries.length));
    this.applyUpTo(this.commitIdx);
    this.save();
    return { term: this.term, ok: true, match: req.prevIndex + req.entries.length, last: this.last };
  }
  /** `verify`: entry k+1 chains to k, and when k is applied each ref event's old value matches the fold. */
  private verify(e: Entry): string | null {
    if (e.prev !== this.lastDigest) return `prev digest mismatch at ${e.index}`;
    if (e.digest !== digestOf({ term: e.term, index: e.index, prev: e.prev, at: e.at, body: e.body })) return `bad digest at ${e.index}`;
    if (e.at < this.lastAt) return `at decreases at ${e.index}`;
    if (e.body.kind === "events" && this.applied === e.index - 1) {
      for (const ev of e.body.events) if (ev.t === "ref" && this.refGet(ev.ref) !== ev.old) return `ref ${ev.ref} old value mismatch at ${e.index}`;
    }
    return null;
  }
  async vote(group: string, req: VoteReq): Promise<VoteRes> {
    this.setGroup(group); this.online();
    if (req.term < this.term) return { term: this.term, granted: false, why: "stale term" };
    if (req.term > this.term) this.stepDown(req.term);
    if (req.lastTerm < this.lastTerm || (req.lastTerm === this.lastTerm && req.lastIndex < this.last)) return { term: this.term, granted: false, why: "log behind" };
    if (this.votedFor && this.votedFor !== req.candidate) return { term: this.term, granted: false, why: "voted" };
    this.votedFor = req.candidate; this.save();
    return { term: this.term, granted: true };
  }
  /** Test-driven election: run for term + 1, then commit a `Noop`. */
  async campaign(group: string): Promise<string> {
    this.setGroup(group); this.online();
    if (this.role === "leader") return `already leader of term ${this.term}`;
    this.term++; this.votedFor = this.self; this.role = "candidate"; this.save();
    const req: VoteReq = { term: this.term, candidate: this.self, lastIndex: this.last, lastTerm: this.lastTerm, transfer: null };
    const votes = await Promise.allSettled(this.others().map((s) => call(() => this.peer(s).vote(this.group, req))));
    const granted = votes.some((v) => v.status === "fulfilled" && v.value.granted);
    const higher = Math.max(0, ...votes.map((v) => (v.status === "fulfilled" ? v.value.term : 0)));
    if (higher > this.term) { this.stepDown(higher); return "lost: higher term"; }
    if (!granted) { this.role = "follower"; this.save(); return "lost"; }
    this.role = "leader"; this.leader = this.self;
    for (const s of this.others()) { this.next[s] = this.last + 1; this.match[s] = 0; this.sentCommit[s] = 0; }
    await this.appendAsLeader({ kind: "noop" }, Math.max(this.cal(), this.lastAt));
    this.note(`leader of term ${this.term}`);
    return `leader of term ${this.term}`;
  }

  // ---- archiving (leader) ----

  private logBytes(): number { return (this.sql.exec<Row>(`SELECT COALESCE(SUM(bytes), 0) AS b FROM log`).one().b as number); }
  /** The next range to archive: committed entries after the last archive, older than LOG_KEEP or over LOG_MAX_BYTES. */
  private archiveRange(): [number, number] | null {
    const start = this.archThrough() + 1, cutoff = this.cal() - LOG_KEEP;
    let over = this.logBytes() - LOG_MAX_BYTES, end = 0;
    for (const r of this.sql.exec<Row>(`SELECT idx, at, bytes FROM log WHERE idx BETWEEN ? AND ? ORDER BY idx`, start,
      Math.min(this.applied, start + ARCHIVE_MAX_ENTRIES - 1)).toArray()) {
      if ((r.at as number) >= cutoff && over <= 0) break;
      end = r.idx as number; over -= r.bytes as number;
    }
    return end >= start ? [start, end] : null;
  }
  private async archive(): Promise<string> {
    const range = this.archiveRange();
    if (!range) return "nothing to archive";
    const [from, to] = range;
    // `Promoted` first: the inline packs the range names become one stored pack (design/DESIGN.md, `Promoted`).
    const inl = this.sql.exec<Row>(`SELECT json FROM inline_objs WHERE idx <= ? ORDER BY oid`, to).toArray().map((r) => JSON.parse(r.json as string) as GitObj);
    if (inl.length) {
      const pack = await placePack(this.env, this.group, { id: packId(this.cal()), cruft: false, objects: inl }, `promote-${from}-${to}`);
      const r = await this.submitOne(`promote-${from}-${to}`, { kind: "promoted", from, to, pack });
      if (!("out" in r) || !r.out.ok) return `promote refused: ${JSON.stringify(r)}`;
    }
    const es = this.entries(from, to);
    const text = archiveText(this.group, from, to, es), digest = await sha256(text);
    const key = R2.archive(this.group, from, to, digest);
    const sites = this.uploadSites();
    const put = await Promise.all(sites.map((s) => putOnce(bucket(this.env, s), key, text, digest, { chain: this.group, term: String(this.term) })));
    if (this.flags.abandonAfterPut) { this.note(`archive ${from}-${to} put to ${sites}, abandoned before Archived`); return `abandoned after put ${from}-${to}`; }
    const rec: ArchivedRec = { kind: "archive", key, from, to, digest, fromAt: es[0].at, toAt: es.at(-1)!.at, sites: [this.self, nextSite(this.self)] };
    return this.commitArchived(rec, put.join("/"));
  }
  /** Puts go to this site's bucket and the next one; `skipSecondPut` loses the second. */
  private uploadSites(): SiteKey[] { return this.flags.skipSecondPut ? [this.self] : [this.self, nextSite(this.self)]; }
  /** The leader commits `Archived` only after HEADing the key in both buckets. */
  private async commitArchived(rec: ArchivedRec, how: string): Promise<string> {
    if (!this.flags.commitWithoutHead) {
      const h = await headIn(this.env, rec.sites, rec.key, rec.digest);
      if (!rec.sites.every((s) => h[s])) { this.note(`not committing ${rec.key}: HEAD ${JSON.stringify(h)}`); return `refused to commit ${rec.kind} ${rec.from}-${rec.to}: HEAD ${JSON.stringify(h)}`; }
    }
    const r = await this.submitOne(`archived-${rec.key}`, { kind: "archived", rec });
    return `${rec.kind} ${rec.from}-${rec.to} (${how}): ${JSON.stringify("out" in r ? r.out : r)}`;
  }
  /** Served by the leader: a follower's snapshot is up; HEAD both buckets and commit `Archived`. */
  async snapshotDone(group: string, rec: ArchivedRec): Promise<string> {
    this.setGroup(group); this.online();
    if (this.role !== "leader") return "not leader";
    const run = this.submitting.then(() => this.commitArchived(rec, "snapshot"));
    this.submitting = run.catch(() => {});
    return run;
  }

  // ---- snapshots (one follower) ----

  private snapThrough(): number { return Math.max(0, ...this.records("snapshot").map((r) => r.to)); }
  private async snapshot(): Promise<string> {
    if (this.role === "leader" || !this.leader || this.applied <= this.base) return "not a snapshot writer";
    const to = this.applied, since = this.snapThrough();
    const lastAt = this.records("snapshot").find((r) => r.to === since)?.toAt ?? 0;
    const due = to - since >= SNAPSHOT_EVERY || (to > since && Math.floor(this.cal() / DAY) !== Math.floor(lastAt / DAY));
    const writers = SITES.filter((s) => s !== this.leader);
    if (!due || writers[Math.floor(to / SNAPSHOT_EVERY) % writers.length] !== this.self) return "not due here";
    // Rows read in one synchronous step, so no `snap_undo` is needed at this size [simplification].
    const e = this.entry(to)!;
    const text = JSON.stringify({ chain: this.group, to, term: e.term, digest: e.digest, at: e.at, rows: this.foldRows() });
    const digest = await sha256(text), key = R2.snapshot(this.group, to, digest);
    await Promise.all(this.uploadSites().map((s) => putOnce(bucket(this.env, s), key, text, digest, { chain: this.group })));
    const rec: ArchivedRec = { kind: "snapshot", key, from: 0, to, digest, fromAt: 0, toAt: e.at, sites: [this.self, nextSite(this.self)] };
    return call(() => this.peer(this.leader!).snapshotDone(this.group, rec), 20_000).catch((err) => `snapshotDone failed: ${err}`);
  }

  // ---- truncation (every member) ----

  /** Truncates through committed `Archived` ranges, in order, whose key this member HEADs in both buckets, and only
   *  ranges older than LOG_KEEP or while the log is over LOG_MAX_BYTES. */
  private async truncate(): Promise<string> {
    if (this.flags.blindTruncate) {
      const through = (this.sql.exec<Row>(`SELECT MAX(idx) AS i FROM log WHERE idx <= ? AND at < ?`, this.applied, this.cal() - LOG_KEEP).one().i as number) ?? 0;
      if (through > this.base) { this.dropThrough(through, "blind", []); return `blind truncate to ${through}`; }
      return "nothing";
    }
    let over = this.logBytes() - LOG_MAX_BYTES;
    const keys: string[] = [];
    let through = this.base;
    for (const r of this.archChain()) {
      if (r.to <= this.base) continue;
      if (r.to > this.applied || (r.toAt >= this.cal() - LOG_KEEP && over <= 0)) break;
      let h = await headIn(this.env, r.sites, r.key, r.digest);
      if (!r.sites.every((s) => h[s])) {
        this.refusedTruncations++;
        this.note(`refused to truncate past ${r.from}-${r.to}: HEAD ${JSON.stringify(h)}`);
        if (this.flags.noRepair || r.from <= this.base) break;
        // Repair: the range is committed and this member holds its entries, so it re-puts the same bytes [inference].
        const text = archiveText(this.group, r.from, r.to, this.entries(r.from, r.to));
        if ((await sha256(text)) !== r.digest) break;
        for (const s of r.sites) if (!h[s]) await putOnce(bucket(this.env, s), r.key, text, r.digest, { chain: this.group, repair: this.self });
        h = await headIn(this.env, r.sites, r.key, r.digest);
        if (!r.sites.every((s) => h[s])) break;
        this.note(`repaired ${r.key}`);
      }
      over -= (this.sql.exec<Row>(`SELECT COALESCE(SUM(bytes), 0) AS b FROM log WHERE idx BETWEEN ? AND ?`, r.from, r.to).one().b as number);
      through = r.to; keys.push(r.key);
    }
    if (through <= this.base) return "nothing to truncate";
    this.dropThrough(through, "truncate", keys);
    return `truncated ${keys.length} ranges to ${through}`;
  }
  private dropThrough(through: number, kind: TruncRow["kind"], keys: string[]): void {
    const e = this.entry(through)!;
    this.sql.exec(`INSERT INTO trunc VALUES (?, ?, ?, ?, ?, ?)`, Date.now(), this.cal(), kind, this.base, through, JSON.stringify(keys));
    this.sql.exec(`DELETE FROM log WHERE idx <= ?`, through);
    this.base = through; this.baseTerm = e.term; this.baseDigest = e.digest; this.baseAt = e.at;
    this.loadTail(); this.save();
  }

  // ---- InstallSnapshot ----

  private installReq(): InstallReq {
    const snaps = this.records("snapshot");
    const core = snaps.length ? snaps.reduce((a, b) => (b.to > a.to ? b : a)) : null;
    const archives = this.archChain().filter((r) => r.to > (core?.to ?? 0) && r.from <= this.base);
    return { term: this.term, leader: this.self, core, archives, through: Math.max(core?.to ?? 0, archives.at(-1)?.to ?? 0) };
  }
  async installSnapshot(group: string, req: InstallReq): Promise<{ term: number; ok: boolean; match: number; why?: string }> {
    this.setGroup(group); this.online();
    if (req.term < this.term) return { term: this.term, ok: false, match: 0, why: "stale term" };
    if (req.term > this.term || this.role !== "follower") this.stepDown(req.term);
    this.leader = req.leader;
    if (this.installing) return { term: this.term, ok: false, match: 0, why: "installing" };
    this.installing = true;
    try {
      const keys = [...(req.core ? [req.core] : []), ...req.archives];
      for (const r of keys) {
        const h = await headIn(this.env, r.sites, r.key, r.digest);
        if (!r.sites.every((s) => h[s])) return { term: this.term, ok: false, match: 0, why: `cannot HEAD ${r.key} in two buckets` };
      }
      const texts = await Promise.all(keys.map((r) => getVerified(this.env, r.sites, r.key, r.digest)));
      if (texts.some((t) => !t)) return { term: this.term, ok: false, match: 0, why: "unreadable object" };
      let applied = 0, digest = ZERO, term = 0, at = 0, i = 0;
      for (const [t] of FOLD) this.sql.exec(`DELETE FROM ${t}`);
      if (req.core) {
        const snap = JSON.parse(texts[i++]!.text) as { to: number; term: number; digest: string; at: number; rows: Record<string, Row[]> };
        for (const [t] of FOLD) for (const row of snap.rows[t] ?? []) {
          const cols = Object.keys(row);
          this.sql.exec(`INSERT INTO ${t} (${cols.join(", ")}) VALUES (${cols.map(() => "?").join(", ")})`, ...cols.map((c) => row[c]));
        }
        applied = snap.to; digest = snap.digest; term = snap.term; at = snap.at;
      }
      this.applied = applied;
      for (; i < texts.length; i++) {
        for (const e of (JSON.parse(texts[i]!.text) as { entries: Entry[] }).entries) {
          if (e.index <= applied) { if (e.index === applied && e.digest !== digest) throw new Error(`archive disagrees with snapshot at ${e.index}`); continue; }
          if (e.index !== applied + 1 || e.prev !== digest || e.digest !== digestOf({ term: e.term, index: e.index, prev: e.prev, at: e.at, body: e.body }))
            throw new Error(`archive chain broken at ${e.index}`);
          this.apply(e); applied = e.index; digest = e.digest; term = e.term; at = e.at;
        }
      }
      this.sql.exec(`INSERT INTO trunc VALUES (?, ?, ?, ?, ?, ?)`, Date.now(), this.cal(), "install", this.base, applied, JSON.stringify(keys.map((k) => k.key)));
      this.sql.exec(`DELETE FROM log`);
      this.base = applied; this.baseTerm = term; this.baseDigest = digest; this.baseAt = at;
      this.applied = applied; this.commitIdx = Math.max(this.commitIdx, applied);
      this.loadTail(); this.save(); this.installs++;
      this.note(`installed snapshot ${req.core?.to ?? "none"} + ${req.archives.length} archives through ${applied}`);
      return { term: this.term, ok: true, match: applied };
    } catch (e) {
      this.note(`install failed: ${String(e).slice(0, 100)}`);
      return { term: this.term, ok: false, match: 0, why: String(e).slice(0, 200) };
    } finally { this.installing = false; }
  }

  // ---- GC (leader, once per calendar day) ----

  /** Roots are the fold refs plus every `ref_log` value. Unreachable objects go to a cruft pack with the time they left
   *  the root set, and expire CRUFT_EXPIRY after it. Compaction writes one geometric pack and one cruft pack, commits
   *  `Compacted`, and only then deletes the consumed packs, at most GC_RATE per run. */
  private async gc(): Promise<string> {
    const day = Math.floor(this.cal() / DAY);
    if (day === this.lastGcDay) return "gc ran today";
    this.lastGcDay = day; this.save();
    const view = this.packView(), now = this.cal();
    const roots = new Set(this.sql.exec<Row>(`SELECT oid FROM refs`).toArray().map((r) => r.oid as string));
    if (variantOf(this.group) !== "noreflog")
      for (const r of this.sql.exec<Row>(`SELECT oid FROM ref_log WHERE oid IS NOT NULL`).toArray()) roots.add(r.oid as string);
    if (!view.stored.length) return "no stored packs";
    const { objs, unreadable } = await loadPacks(this.env, this.group, { stored: view.stored, inline: [] });
    if (unreadable.length) { this.note(`gc: unreadable packs ${unreadable}`); return `gc skipped: ${unreadable.length} unreadable`; }
    const inlineOids = new Set(view.inline.map((o) => o.oid));
    const all = new Map(objs); for (const o of view.inline) if (!all.has(o.oid)) all.set(o.oid, o);
    const { reached } = closure(all, [...roots]);
    const live = new Set(reached.map((o) => o.oid));
    const geo: GitObj[] = [], cruft: (GitObj & { since: number })[] = [];
    let expired = 0;
    for (const o of objs.values()) {
      if (live.has(o.oid)) { geo.push({ oid: o.oid, type: o.type, data: o.data, links: o.links }); continue; }
      if (inlineOids.has(o.oid)) continue;
      const since = o.since ?? now;
      if (since < now - CRUFT_EXPIRY) { expired++; continue; }
      cruft.push({ oid: o.oid, type: o.type, data: o.data, links: o.links, since });
    }
    const changed = view.stored.length > 2 || expired > 0 || cruft.some((o) => objs.get(o.oid)?.since === undefined);
    let summary = `gc day ${day}: ${roots.size} roots, ${live.size} live, ${cruft.length} cruft, ${expired} expired`;
    if (changed) {
      const produced: StoredPack[] = [];
      if (geo.length) produced.push(await placePack(this.env, this.group, { id: packId(now), cruft: false, objects: geo }, `gc-${day}`, "geo"));
      if (cruft.length) produced.push(await placePack(this.env, this.group, { id: packId(now), cruft: true, objects: cruft }, `gc-${day}`, "cruft"));
      const r = await this.submitOne(`gc-${day}-${view.at}`, { kind: "compacted", consumed: view.stored.map((p) => p.id), produced, readAt: view.at });
      summary += `; Compacted ${view.stored.length} -> ${produced.length}: ${JSON.stringify("out" in r ? r.out : r)}`;
      if (expired) this.bump("gc_expired", expired);
    }
    // Physical deletes of packs a committed `Compacted` consumed, at a capped rate.
    const del = this.sql.exec<Row>(`SELECT id FROM gc_delete LIMIT ?`, GC_RATE).toArray().map((r) => r.id as string);
    for (const id of del) {
      await Promise.all(SITES.map((s) => bucket(this.env, s).delete([R2.pack(this.group, id), R2.committed(this.group, id)])));
      this.sql.exec(`DELETE FROM gc_delete WHERE id = ?`, id);
    }
    this.note(`${summary}; deleted ${del.length} packs`);
    return summary;
  }

  // ---- G6: GC raced against pushes and restores ----

  /** One GC, split so writes can commit between its root read and its `Compacted`: read roots and the pack set at
   *  `readAt`, plan, place the geometric and cruft packs, wait `holdMs` (the race window), then submit `Compacted`.
   *  On a refusal for changed refs: if nothing it would expire became reachable from the current roots, resubmit the same
   *  packs with the new `readAt` (a cheap rerun); otherwise replan from scratch (a full rerun). The deletes of consumed
   *  packs run only after a committed `Compacted`, as in `gc()`. Not limited to once a day. */
  async gcRace(group: string, holdMs: number): Promise<GcRace> {
    this.setGroup(group); this.online();
    const t0 = Date.now(), g = guardsOf(group);
    const res: GcRace = { readAt: [], committed: false, attempts: 0, cheapReruns: 0, fullReruns: 0, expired: 0, savedByRerun: 0, refusals: [], ms: 0, consumed: 0, orphanPacks: 0, entry: 0 };
    this.lastGcDay = Math.floor(this.cal() / DAY); this.save(); // this is the day's GC; `tick` skips its own
    if (this.role !== "leader") { res.refusals.push("not leader"); return res; }
    let hold = holdMs;
    for (let full = 0; full < 6 && !res.committed; full++) {
      if (full) res.fullReruns++;
      // Read and plan. Reads happen between commits, because `submitting` is not held here.
      const now = this.cal(), view = this.packView();
      const roots = this.gcRoots();
      if (!view.stored.length) { res.refusals.push("no stored packs"); break; }
      const { objs, unreadable } = await loadPacks(this.env, this.group, { stored: view.stored, inline: [] });
      if (unreadable.length) { res.refusals.push(`unreadable ${unreadable.length}`); break; }
      const all = new Map(objs); for (const o of view.inline) if (!all.has(o.oid)) all.set(o.oid, o);
      const live = new Set(closure(all, roots).reached.map((o) => o.oid));
      const geo: GitObj[] = [], cruft: (GitObj & { since: number })[] = [], expiring: string[] = [];
      for (const o of objs.values()) {
        if (live.has(o.oid)) { geo.push({ oid: o.oid, type: o.type, data: o.data, links: o.links }); continue; }
        const since = o.since ?? now;
        if (since < now - CRUFT_EXPIRY) { expiring.push(o.oid); continue; }
        cruft.push({ oid: o.oid, type: o.type, data: o.data, links: o.links, since });
      }
      const produced: StoredPack[] = [];
      if (geo.length) produced.push(await placePack(this.env, this.group, { id: packId(now), cruft: false, objects: geo }, `gcr-${t0}-${full}`, "geo"));
      if (cruft.length) produced.push(await placePack(this.env, this.group, { id: packId(now), cruft: true, objects: cruft }, `gcr-${t0}-${full}`, "cruft"));
      if (hold > 0) { await sleep(hold); hold = 0; }
      let readAt = view.at;
      for (let cheap = 0; cheap < 10; cheap++) {
        res.attempts++; res.readAt.push(readAt);
        const r = await this.submit(group, `gcr-${t0}-${full}-${cheap}`, { kind: "compacted", consumed: view.stored.map((p) => p.id), produced, readAt });
        if ("out" in r && r.out.ok) { res.committed = true; res.expired = expiring.length; res.consumed = view.stored.length; res.entry = r.entry; break; }
        const why = "out" in r ? r.out.why ?? "" : "not leader";
        res.refusals.push(why);
        if (!why.startsWith("refs changed") || !g.cheapRerun) break;
        // Cheap rerun: recheck only whether an object this plan expires is reachable from the roots as they are now.
        // Everything the plan keeps is in `produced`, so if none of the expiring objects is reachable the plan still holds.
        this.applyUpTo(this.commitIdx);
        const nowAt = this.applied;
        const cur = this.packView(), seen = new Map(all);
        for (const o of cur.inline) if (!seen.has(o.oid)) seen.set(o.oid, o);
        const reach = new Set(closure(seen, this.gcRoots()).reached.map((o) => o.oid));
        const needed = expiring.filter((o) => reach.has(o));
        // A new stored pack could link anywhere; replan rather than read it.
        if (needed.length || cur.stored.map((p) => p.id).join() !== view.stored.map((p) => p.id).join()) {
          res.savedByRerun += needed.length; break;
        }
        res.cheapReruns++; readAt = nowAt;
      }
      if (!res.committed) res.orphanPacks += produced.length; // placed but never named; the sweeper removes them
    }
    if (res.committed) {
      const del = this.sql.exec<Row>(`SELECT id FROM gc_delete`).toArray().map((r) => r.id as string);
      for (const id of del) {
        await Promise.all(SITES.map((s) => bucket(this.env, s).delete([R2.pack(this.group, id), R2.committed(this.group, id)])));
        this.sql.exec(`DELETE FROM gc_delete WHERE id = ?`, id);
      }
    }
    res.ms = Date.now() - t0;
    this.note(`gcRace: ${JSON.stringify({ ...res, refusals: res.refusals.slice(0, 3) })}`);
    return res;
  }
  /** After a race: every object reachable from `refs`, and from `ref_log` values, over the live packs read from R2.
   *  `present` says whether each named oid is in a live pack. */
  async lostCheck(group: string, oids: string[]): Promise<{ applied: number; refsMissing: string[]; reflogMissing: string[]; reached: number; present: Record<string, boolean> }> {
    this.setGroup(group); this.online();
    this.applyUpTo(this.commitIdx);
    const applied = this.applied, view = this.packView();
    const refs = this.sql.exec<Row>(`SELECT oid FROM refs`).toArray().map((r) => r.oid as string);
    const reflog = this.sql.exec<Row>(`SELECT DISTINCT oid FROM ref_log WHERE oid IS NOT NULL`).toArray().map((r) => r.oid as string);
    const { objs } = await loadPacks(this.env, this.group, view);
    const a = closure(objs, refs), b = closure(objs, reflog);
    return { applied, refsMissing: a.missing, reflogMissing: b.missing, reached: b.reached.length, present: Object.fromEntries(oids.map((o) => [o, objs.has(o)])) };
  }
  /** Dry run of the next GC's plan: the oids it would expire now. No writes. */
  async gcPlan(group: string): Promise<{ readAt: number; expiring: string[]; cruft: number }> {
    this.setGroup(group); this.online();
    this.applyUpTo(this.commitIdx);
    const now = this.cal(), view = this.packView();
    const { objs } = await loadPacks(this.env, this.group, { stored: view.stored, inline: [] });
    const all = new Map(objs); for (const o of view.inline) if (!all.has(o.oid)) all.set(o.oid, o);
    const live = new Set(closure(all, this.gcRoots()).reached.map((o) => o.oid));
    const expiring: string[] = []; let cruft = 0;
    for (const o of objs.values()) if (!live.has(o.oid)) { if ((o.since ?? now) < now - CRUFT_EXPIRY) expiring.push(o.oid); else cruft++; }
    return { readAt: view.at, expiring, cruft };
  }
  private gcRoots(): string[] {
    const roots = new Set(this.sql.exec<Row>(`SELECT oid FROM refs`).toArray().map((r) => r.oid as string));
    if (variantOf(this.group) !== "noreflog")
      for (const r of this.sql.exec<Row>(`SELECT oid FROM ref_log WHERE oid IS NOT NULL`).toArray()) roots.add(r.oid as string);
    return [...roots];
  }
  /** The push side of receive-pack's connectivity check for a push that sends no objects: every new ref target's closure
   *  must be in the live packs at the pack-set index read. The push carries that index as `readAt`. */
  async pushExisting(group: string, rid: string, updates: RefUpdate[], holdMs = 0): Promise<SubmitRes & { check?: { readAt: number; missing: number; unreadable: number } }> {
    this.setGroup(group); this.online();
    if (this.role !== "leader") return { notLeader: { term: this.term, leader: this.leader } };
    this.applyUpTo(this.commitIdx);
    const view = this.packView();
    const { objs, unreadable } = await loadPacks(this.env, this.group, view);
    const { missing } = closure(objs, updates.map((u) => u.new).filter((x): x is string => !!x));
    const check = { readAt: view.at, missing: missing.length, unreadable: unreadable.length };
    if (unreadable.length) return { out: { ok: false, why: `connectivity: ${unreadable.length} packs unreadable (retry)` }, entry: 0, check };
    if (missing.length) return { out: { ok: false, why: `connectivity: ${missing.length} objects missing` }, entry: 0, check };
    if (holdMs > 0) await sleep(holdMs);
    const r = await this.submit(group, rid, { kind: "push", updates, pack: null, readAt: view.at });
    return { ...r, check };
  }

  /** The orphan sweeper: a pack with no `Committed` marker, older than ORPHAN_AGE, not live, whose request id never
   *  committed, is deleted from every bucket. A committed one gets its missing marker. */
  private async sweep(): Promise<string> {
    const now = this.cal(), live = new Set(this.packView().stored.map((p) => p.id));
    let deleted = 0, marked = 0;
    for (const s of SITES) {
      const b = bucket(this.env, s);
      const list = await b.list({ prefix: `pack/${this.group}/`, include: ["customMetadata"] });
      for (const o of list.objects) {
        const id = o.key.split("/").pop()!;
        if (live.has(id) || now - packTime(id) < ORPHAN_AGE_MS) continue;
        if (await b.head(R2.committed(this.group, id))) continue;
        const rid = o.customMetadata?.rid ?? "";
        if (this.sql.exec(`SELECT 1 FROM req WHERE rid = ?`, rid).toArray().length) { await b.put(R2.committed(this.group, id), "{}"); marked++; continue; }
        if (this.sql.exec(`SELECT 1 FROM gc_delete WHERE id = ?`, id).toArray().length) continue;
        await b.delete(o.key); deleted++;
      }
    }
    if (deleted || marked) this.note(`sweep: deleted ${deleted} orphan pack objects, marked ${marked}`);
    this.bump("swept", deleted);
    return `swept ${deleted}, marked ${marked}`;
  }

  // ---- RestoredTo (leader) ----

  /** The leader's reads before `decide`: `at` of entry `to` (from the live log, or from the archive that holds it), the
   *  targets from `ref_log`, and the reachability walk over the live and cruft packs with each object's SHA-1 checked. */
  async restore(group: string, rid: string, to: number, refs: string[], holdMs = 0): Promise<SubmitRes & { check?: RestoreCheck }> {
    this.setGroup(group); this.online();
    if (this.role !== "leader") return { notLeader: { term: this.term, leader: this.leader } };
    let toAt: number | undefined, via = "log";
    if (to > this.base) toAt = this.entry(to)?.at;
    else {
      const r = this.archChain().find((x) => x.from <= to && to <= x.to);
      const t = r && (await getVerified(this.env, r.sites, r.key, r.digest));
      toAt = t ? (JSON.parse(t.text) as { entries: Entry[] }).entries.find((e) => e.index === to)?.at : undefined;
      via = r ? `archive ${r.key} from ${t?.site}` : "no archive";
    }
    if (toAt === undefined) return { out: { ok: false, why: `cannot read entry ${to} (${via})` }, entry: 0 };
    const view = this.packView();
    const targets = this.refLogAt(to, refs);
    const { objs, unreadable } = await loadPacks(this.env, this.group, view);
    const { reached, missing } = closure(objs, Object.values(targets).filter((x): x is string => !!x));
    for (const o of reached) if (!(await objectOk(o))) missing.push(`bad sha1 ${o.oid}`);
    const check = { toAt, targets, objects: reached.length, missing, unreadable, via, readAt: view.at };
    if (holdMs > 0) await sleep(holdMs);
    const res = await this.submit(group, rid, { kind: "restore", to, refs, check });
    return { ...res, check };
  }

  /** `to` for a time t: the last index with `at <= t` (design/DESIGN.md, `RepoRestore`). When that entry is truncated it
   *  is found through the committed archive records' `fromAt`, which `Archived` does not carry in the design. */
  async indexAt(group: string, t: number): Promise<{ to: number; at: number; via: string } | { error: string }> {
    this.setGroup(group); this.online();
    const live = this.sql.exec<Row>(`SELECT idx, at FROM log WHERE at <= ? ORDER BY idx DESC LIMIT 1`, t).toArray()[0];
    if (live && (live.idx as number) > this.base) return { to: live.idx as number, at: live.at as number, via: "log" };
    if (this.base === 0) return { error: "t is before the first entry" };
    const r = this.archChain().filter((x) => x.fromAt <= t).at(-1);
    if (!r) return { error: "no archive starts at or before t" };
    const text = await getVerified(this.env, r.sites, r.key, r.digest);
    if (!text) return { error: `archive ${r.key} unreadable` };
    const e = (JSON.parse(text.text) as { entries: Entry[] }).entries.filter((x) => x.at <= t).at(-1)!;
    return { to: e.index, at: e.at, via: `archive ${r.from}-${r.to} from bucket ${text.site}` };
  }

  // ---- maintenance and test control ----

  /** One maintenance pass, as the cron and alarms would run it. */
  async tick(group: string): Promise<string[]> {
    this.setGroup(group); this.online();
    const out: string[] = [];
    const step = async (name: string, f: () => Promise<string>) => { try { out.push(`${name}: ${await f()}`); } catch (e) { out.push(`${name} failed: ${String(e).slice(0, 120)}`); } };
    if (this.role === "leader") {
      const run = this.submitting.then(async () => {
        for (let i = 0; i < 20; i++) { const r = await this.archive(); out.push(`archive: ${r}`); if (!/: \{"ok":true/.test(r)) break; }
        await step("gc", () => this.gc());
        await step("sweep", () => this.sweep());
      });
      this.submitting = run.catch(() => {});
      await run;
      for (const s of this.others()) void this.replicate(s);
    } else await step("snapshot", () => this.snapshot());
    await step("truncate", () => this.truncate());
    return out;
  }
  async setClock(group: string, calMs: number): Promise<void> { this.setGroup(group); this.offset = calMs - Date.now(); this.save(); }
  async setFlags(group: string, flags: Flags): Promise<Flags> { this.setGroup(group); this.flags = flags; this.save(); return flags; }
  async refs(group: string): Promise<{ refs: Record<string, string>; applied: number; leader: boolean }> {
    this.setGroup(group); this.online();
    return { refs: Object.fromEntries(this.sql.exec<Row>(`SELECT ref, oid FROM refs`).toArray().map((r) => [r.ref, r.oid])), applied: this.applied, leader: this.role === "leader" };
  }
  /** Read-only views for the edge's checks; served even while `offline`. */
  async view(group: string): Promise<{ status: MemberStatus; archived: ArchivedRec[]; packs: PackView; trunc: TruncRow[]; meta: Record<string, number> }> {
    this.setGroup(group);
    const trunc = this.sql.exec<Row>(`SELECT * FROM trunc ORDER BY real`).toArray().map((r) => ({ real: r.real as number, cal: r.cal as number,
      kind: r.kind as TruncRow["kind"], before: r.before as number, after: r.after as number, keys: JSON.parse(r.keys as string) }));
    const meta = Object.fromEntries(this.sql.exec<Row>(`SELECT k, v FROM fmeta UNION ALL SELECT k, v FROM lmeta`).toArray().map((r) => [r.k, r.v as number]));
    return { status: this.status(), archived: [...this.records("archive"), ...this.records("snapshot")], packs: this.packView(), trunc, meta };
  }
  status(): MemberStatus {
    return { site: this.self, role: this.role, term: this.term, leader: this.leader, last: this.last, commit: this.commitIdx, applied: this.applied,
      base: this.base, cal: this.cal(), flags: this.flags, foldDigest: sha256hex(JSON.stringify(this.foldRows())), logBytes: this.logBytes(),
      archThrough: this.archThrough(), snapThrough: this.snapThrough(), refusedTruncations: this.refusedTruncations, installs: this.installs,
      notes: [...this.notes] };
  }
}

const ORPHAN_AGE_MS = DAY;

export default { async fetch(): Promise<Response> { return new Response("not found", { status: 404 }); } } satisfies ExportedHandler<MemberEnv>;
