// M0-WP12: the core-tier view `decideX` reads, over a member's SQLite core tables plus an overlay of the events of
// entries not yet applied and of earlier commands in the entry being built (design/DESIGN.md, "Invariants": `decide`
// sees the fold plus the earlier commands of the same entry). A fork group's view reads the task, `AgentPolicy`, leases,
// and `Authz` rows from `deps`, which its leader read once from the base group through `fresh` (design/AGENT-LAYER.md,
// `Push::deps()`). Nothing here can read the metadata tier: there is no method for it.

import type { Extend } from "./types.ts";
import { holderKey, type AuthzRows, type CoreView, type Enforce, type Lease, type MergeView, type Task, type XEv } from "./lease.ts";

/** What a fork group's leader reads from its base group for one entry. */
export interface Deps { tasks: Record<string, Task | null>; policy: Enforce; leases: Lease[]; authz: AuthzRows;
  collabs: Record<string, "read" | "write" | "admin"> }

type Pull = { head: string; state: "open" | "merged" | "closed"; agentApp: number | null };

export class XView implements CoreView, MergeView {
  private leases = new Map<string, Lease | null>();
  /** Leases acquired in the overlay (not in SQL), so counts need no SQL read for them. */
  private created = new Set<string>();
  /** `ExtendLeases` of unapplied entries and of the entry being built, in log order. SQL rows predate all of them. */
  private exts: Extend[] = [];
  private rows = new Map<string, unknown>();
  private policyV: Enforce | null = null;
  readonly authz: AuthzRows;
  /** Core-tier SQL reads made, for the L0-16 counters. */
  reads = 0;

  constructor(private sql: SqlStorage, ext: Extend | null, private kv: (k: string) => string | null, private deps: Deps | null) {
    if (ext) this.exts.push(ext);
    if (deps) this.authz = structuredClone(deps.authz);
    else {
      this.authz = { installs: {}, agentApps: {} };
      for (const r of sql.exec<{ t: string; k: string; j: string }>(`SELECT t, k, j FROM x_row WHERE t IN ('install', 'agentapp')`)) {
        if (r.t === "install") this.authz.installs[r.k] = JSON.parse(r.j); else this.authz.agentApps[r.k] = JSON.parse(r.j);
      }
    }
  }

  private fix(l: Lease): Lease {
    let expires = l.expires;
    for (const x of this.exts) if (expires > x.since) expires += x.by;
    return expires === l.expires ? l : { ...l, expires };
  }
  /** An entry's `ExtendLeases`, in log order: moves SQL rows (through `fix`) and overlay leases alike. */
  extend(x: Extend): void {
    this.exts.push(x);
    for (const [id, l] of this.leases) if (l && l.expires > x.since) this.leases.set(id, { ...l, expires: l.expires + x.by });
  }
  private row<T>(t: string, k: string): T | null {
    const key = `${t}|${k}`;
    if (this.rows.has(key)) return this.rows.get(key) as T | null;
    this.reads++;
    const r = this.sql.exec<{ j: string }>(`SELECT j FROM x_row WHERE t = ? AND k = ?`, t, k).toArray()[0];
    const v = r ? (JSON.parse(r.j) as T) : null;
    this.rows.set(key, v);
    return v;
  }
  /** Leases matching a SQL filter, with the overlay applied and the entry's own `extend`. */
  private leaseQuery(where: string, args: (string | number)[], keep: (l: Lease) => boolean): Lease[] {
    this.reads++;
    const out = new Map<string, Lease>();
    for (const r of this.sql.exec<{ j: string; expires: number }>(`SELECT j, expires FROM x_lease WHERE ${where}`, ...args)) {
      const l = this.fix({ ...(JSON.parse(r.j) as Lease), expires: r.expires });
      if (!this.leases.has(l.id) && keep(l)) out.set(l.id, l);
    }
    for (const l of this.leases.values()) if (l && keep(l)) out.set(l.id, l);
    return [...out.values()];
  }
  /** `expires` is compared after the entry's `extend`, so the SQL bound subtracts its `by`. */
  private get slack(): number { return this.exts.reduce((a, x) => a + x.by, 0); }

  task(session: string): Task | null {
    if (this.deps) return this.deps.tasks[session] ?? this.row<Task>("task", session);
    return this.row<Task>("task", session);
  }
  collab(user: number) { return this.deps ? this.deps.collabs[user] ?? null : this.row<"read" | "write" | "admin">("collab", String(user)); }
  policy(): Enforce { return this.deps?.policy ?? this.policyV ?? this.row<Enforce>("policy", "leases") ?? "Advisory"; }
  lease(id: string): Lease | null {
    if (this.leases.has(id)) return this.leases.get(id)!;
    this.reads++;
    const r = this.sql.exec<{ j: string; expires: number }>(`SELECT j, expires FROM x_lease WHERE id = ?`, id).toArray()[0];
    return r ? this.fix({ ...(JSON.parse(r.j) as Lease), expires: r.expires }) : null;
  }
  exclusiveNear(ancestors: string[], under: string[], at: number): Lease[] {
    const near = (l: Lease) => l.mode === "Exclusive" && l.expires > at;
    if (this.deps) return this.deps.leases.filter(near);
    if (!ancestors.length && !under.length) return [];
    const conds = [...ancestors.map(() => "px = ?"), ...under.map(() => "px LIKE ? ESCAPE '\\'")];
    const args = [...ancestors, ...under.map((u) => `${u.replace(/[\\%_]/g, (c) => `\\${c}`)}/%`)];
    const ids = `SELECT id FROM x_lease_px WHERE ${conds.join(" OR ")}`;
    const set = new Set(ancestors), unders = under.map((u) => `${u}/`);
    return this.leaseQuery(`mode = 'Exclusive' AND expires > ? AND id IN (${ids})`, [at - this.slack, ...args],
      (l) => near(l) && l.paths.some((p) => { const lp = literal(p); return set.has(lp) || unders.some((u) => lp.startsWith(u)); }));
  }
  holderLeases(holder: string, at: number): Lease[] {
    if (holder.startsWith("*session:")) {
      const s = holder.slice(9);
      return this.leaseQuery(`session = ? AND expires > ?`, [s, at - this.slack], (l) => l.holder.session === s && l.expires > at);
    }
    return this.leaseQuery(`hkey = ? AND expires > ?`, [holder, at - this.slack], (l) => holderKey(l.holder) === holder && l.expires > at);
  }
  private count(where: string, args: (string | number)[], keep: (l: Lease) => boolean, at: number): number {
    this.reads++;
    let n = this.sql.exec<{ n: number }>(`SELECT count(*) AS n FROM x_lease WHERE ${where} AND expires > ?`, ...args, at - this.slack).one().n;
    for (const [id, l] of this.leases) {
      const was = this.created.has(id) ? 0 : this.sql.exec<{ n: number }>(`SELECT count(*) AS n FROM x_lease WHERE id = ? AND ${where} AND expires > ?`, id, ...args, at - this.slack).one().n;
      n += (l && keep(l) && l.expires > at ? 1 : 0) - was;
    }
    return n;
  }
  countApp(app: number, at: number) { return this.count(`app = ?`, [app], (l) => l.holder.app === app, at); }
  countPlan(at: number) { return this.count(`plan = 1`, [], (l) => !!l.plan, at); }
  countRepo(at: number) { return this.count(`1 = 1`, [], () => true, at); }

  pull(n: number) { return this.row<Pull>("pull", String(n)); }
  approvedOn(n: number, commit: string) { return this.row<number>("review", `${n}|${commit}`) !== null; }
  checksGreen(sha: string) { return this.row<string>("check", sha) === "success"; }
  gate(n: number, head: string) { return this.row<number>("gate", `${n}|${head}`) !== null; }
  ref(k: string): string | null { return this.rows.has(`ref|${k}`) ? (this.rows.get(`ref|${k}`) as string | null) : this.kv(k); }

  /** Overlays events, as `apply` will write them. */
  push(evs: XEv[]): void {
    for (const e of evs) {
      switch (e.t) {
        case "LeaseAcquire": this.leases.set(e.lease.id, e.lease); this.created.add(e.lease.id); break;
        case "LeaseRenew": { const l = this.lease(e.id); if (l) this.leases.set(e.id, { ...l, expires: e.expires }); break; }
        case "LeaseRelease": for (const id of e.ids) this.leases.set(id, null); break;
        case "TaskCreated": this.rows.set(`task|${e.task.session}`, e.task); break;
        case "TaskUpdated": { const t = this.task(e.session); if (t) this.rows.set(`task|${e.session}`, { ...t, state: e.state }); break; }
        case "Collab": this.rows.set(`collab|${e.user}`, e.perm); break;
        case "Policy": this.policyV = e.leases; break;
        case "Install": this.authz.installs[e.installation] = { app: e.app, active: e.active, push: e.push }; break;
        case "AgentApp": this.authz.agentApps[e.row.app] = e.row; break;
        case "PrOpened": this.rows.set(`pull|${e.n}`, { head: e.head, state: "open", agentApp: e.agentApp }); break;
        case "Review": if (e.state === "APPROVED") this.rows.set(`review|${e.n}|${e.commit}`, e.reviewer); break;
        case "Check": this.rows.set(`check|${e.sha}`, e.conclusion); break;
        case "Gate": this.rows.set(`gate|${e.n}|${e.head}`, 1); break;
        case "Merged": { const p = this.pull(e.n); if (p) this.rows.set(`pull|${e.n}`, { ...p, state: "merged" }); break; }
        case "Ref": this.rows.set(`ref|${e.ref}`, e.value); break;
        case "Meta": break;
      }
    }
  }
}

function literal(glob: string): string {
  const out: string[] = [];
  for (const s of glob.split("/").filter(Boolean)) { if (s.includes("*")) break; out.push(s); }
  return out.join("/");
}

/** Writes events to the core tables, and metadata events to `x_meta` when `meta` is true. Returns the feed's view of
 *  them: paths and kinds (design/AGENT-LAYER.md, `ActivityEvent`). */
export function applyEvents(sql: SqlStorage, evs: XEv[], index: number, meta: boolean): { paths: string[]; kinds: string[]; metaRows: number } {
  const paths: string[] = [], kinds: string[] = [];
  let metaRows = 0;
  const put = (t: string, k: string, v: unknown) => sql.exec(`INSERT OR REPLACE INTO x_row VALUES (?, ?, ?)`, t, k, JSON.stringify(v));
  const getRow = <T>(t: string, k: string) => { const r = sql.exec<{ j: string }>(`SELECT j FROM x_row WHERE t = ? AND k = ?`, t, k).toArray()[0]; return r ? (JSON.parse(r.j) as T) : null; };
  for (const e of evs) {
    switch (e.t) {
      case "LeaseAcquire": {
        const l = e.lease;
        sql.exec(`INSERT OR REPLACE INTO x_lease VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`, l.id, holderKey(l.holder), l.holder.session, l.holder.app,
          l.mode, l.granted, l.expires, e.plan ? 1 : 0, JSON.stringify(l));
        for (const p of new Set(l.paths.map(literal))) sql.exec(`INSERT OR IGNORE INTO x_lease_px VALUES (?, ?)`, p, l.id);
        paths.push(...l.paths); kinds.push("Lease:Acquired");
        break;
      }
      case "LeaseRenew": sql.exec(`UPDATE x_lease SET expires = ? WHERE id = ?`, e.expires, e.id); kinds.push("Lease:Renewed"); break;
      case "LeaseRelease":
        for (const id of e.ids) {
          const r = sql.exec<{ j: string }>(`SELECT j FROM x_lease WHERE id = ?`, id).toArray()[0];
          if (r) paths.push(...(JSON.parse(r.j) as Lease).paths);
          sql.exec(`DELETE FROM x_lease WHERE id = ?`, id); sql.exec(`DELETE FROM x_lease_px WHERE id = ?`, id);
        }
        kinds.push("Lease:Released");
        break;
      case "TaskCreated": put("task", e.task.session, e.task); kinds.push("Task"); break;
      case "TaskUpdated": { const t = getRow<Task>("task", e.session); if (t) put("task", e.session, { ...t, state: e.state }); kinds.push("Task"); break; }
      case "Collab": if (e.perm) put("collab", String(e.user), e.perm); else sql.exec(`DELETE FROM x_row WHERE t = 'collab' AND k = ?`, String(e.user)); break;
      case "Policy": put("policy", "leases", e.leases); break;
      case "Install": put("install", String(e.installation), { app: e.app, active: e.active, push: e.push }); break;
      case "AgentApp": put("agentapp", String(e.row.app), e.row); break;
      case "PrOpened": put("pull", String(e.n), { head: e.head, state: "open", agentApp: e.agentApp }); break;
      case "Review": if (e.state === "APPROVED") put("review", `${e.n}|${e.commit}`, e.reviewer); break;
      case "Check": put("check", e.sha, e.conclusion); break;
      case "Gate": put("gate", `${e.n}|${e.head}`, 1); break;
      case "Merged": { const p = getRow<Pull>("pull", String(e.n)); if (p) put("pull", String(e.n), { ...p, state: "merged" }); kinds.push("AgentPr"); break; }
      case "Ref":
        sql.exec(`INSERT OR REPLACE INTO kv VALUES (?, ?)`, e.ref, e.value);
        if (e.changed?.length) { paths.push(...e.changed); kinds.push("AgentPush"); }
        break;
      case "Meta":
        if (meta) { sql.exec(`INSERT INTO x_meta (idx, kind, body) VALUES (?, ?, ?)`, index, e.kind, e.body); metaRows++; }
        break;
    }
  }
  return { paths, kinds, metaRows };
}
