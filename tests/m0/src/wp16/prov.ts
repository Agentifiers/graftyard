// M0-WP16 L0-10: provenance through a vault restore. Script `graft-wp16-prov`: an edge and one Durable Object class,
// `ProvRepo`, one object per install's repository group: "mono" is the install, "mono-r" the install rebuilt from the
// vault. Each group has its own two site buckets (INST_A/B, REST_A/B), so the rebuilt install shares nothing with the
// one it replaces.
//
// The group is one member with a digest-chained log (design/DESIGN.md, `LogEntry.prev`), a SQLite fold, snapshots
// every SNAP_EVERY entries in two buckets, and each push's objects as one stored pack in two buckets with a SHA-256
// `checksum` (`PackRef::Stored`). Raft replication is M0-WP8's subject and is left out here.
//
// A push runs `validate` (every object's SHA-1, then the trailer and `refs/notes/ai` parser copied from M0-WP10) and
// `decide` (ref compare-and-swap; `Source::Session` linked only for an App installation token holding the named
// `AgentTask`; host-owned session refs written only by `graftyard-workspace` with that task) (design/AGENT-LAYER.md,
// Shape, `Provenance`, `SESSION_REFS`). `resolve` answers what each commit resolves to (journey A6): its session,
// whose host-owned ref and stored log must read back, or its PR through `Merged` or the PR on its ref.
// The vault side is src/wp16/vault.ts; the export endpoints it pulls are here, behind their own token.

import { DurableObject } from "cloudflare:workers";
import { b64, commitHeader, oidOk, sha256hex, treeEntries, type RawObj } from "./gitobj.ts";
import { provenanceOf, sessionFromUrl, type NoteEntry, type RawCommit } from "./provenance.ts";
import type { Source } from "./prov-model.ts";

export interface Env {
  REPO: DurableObjectNamespace<ProvRepo>;
  INST_A: R2Bucket; INST_B: R2Bucket; REST_A: R2Bucket; REST_B: R2Bucket;
  M0_TOKEN: string; EXPORT_TOKEN: string;
}

export const HOST = "graftyard.test", REPO_NAME = "o/mono";
const SNAP_EVERY = 1000;
const ZERO = "0".repeat(64);
const SESSION_REFS = "refs/graftyard/sessions/";
/** The install's `AgentApp`s in this replay; `notes-bot` stands for an unconfined App that is not one. */
const AGENT_APPS = new Set(["graftyard-run", "claude", "vendor-x", "graftyard-workspace"]);

export type Actor = { t: "user"; user: string } | { t: "app"; app: string; task?: string };
export type Cmd =
  | { k: "push" | "merge"; ref: string; old: string | null; new: string | null; actor: Actor; objects: RawObj[]; n?: number }
  | { k: "task"; id: string; app: string; ref: string }
  | { k: "pr"; n: number; ref: string; head: string; meta: { app: string | null; session: string | null } };
type Ev =
  | { t: "ref"; ref: string; oid: string | null }
  | { t: "task"; id: string; app: string; ref: string }
  | { t: "pr"; n: number; ref: string; app: string | null; session: string | null; head: string }
  | { t: "merged"; n: number; commit: string }
  | { t: "prov"; commit: string; ref: string | null; sources: Source[]; session: string | null }
  | { t: "pack"; id: string; checksum: string; bytes: number; oids: string[] };
export interface Entry { idx: number; prev: string; digest: string; at: number; body: string /* JSON Ev[] */ }

const TABLES: Record<string, string> = {
  refs: "ref TEXT PRIMARY KEY, oid TEXT", tasks: "id TEXT PRIMARY KEY, app TEXT, ref TEXT",
  prs: "n INTEGER PRIMARY KEY, ref TEXT, app TEXT, session TEXT, head TEXT, merged TEXT",
  prov: "commit_ TEXT PRIMARY KEY, ref TEXT, sources TEXT, session TEXT",
  packs: "id TEXT PRIMARY KEY, checksum TEXT, bytes INTEGER", objs: "oid TEXT PRIMARY KEY, pack TEXT",
};
const FOLD = Object.keys(TABLES);
type Row = Record<string, string | number | null>;
const digestOf = (prev: string, idx: number, at: number, body: string) => sha256hex(`${prev}|${idx}|${at}|${body}`);

export class ProvRepo extends DurableObject<Env> {
  sql = this.ctx.storage.sql;
  group = "";
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.schema();
  }
  schema(): void {
    for (const [t, cols] of Object.entries(TABLES)) this.sql.exec(`CREATE TABLE IF NOT EXISTS ${t} (${cols})`);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS log (idx INTEGER PRIMARY KEY, prev TEXT, digest TEXT, at INTEGER, body TEXT)`);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT)`);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS snaps (key TEXT PRIMARY KEY, upto INTEGER, digest TEXT)`);
    this.sql.exec(`CREATE TABLE IF NOT EXISTS copied (id TEXT PRIMARY KEY)`);
    this.sql.exec(`CREATE INDEX IF NOT EXISTS prs_ref ON prs (ref)`);
    this.sql.exec(`CREATE INDEX IF NOT EXISTS prs_merged ON prs (merged)`);
  }
  meta(k: string): string | null { return (this.sql.exec<Row>(`SELECT v FROM meta WHERE k = ?`, k).toArray()[0]?.v as string) ?? null; }
  setMeta(k: string, v: string): void { this.sql.exec(`INSERT OR REPLACE INTO meta VALUES (?, ?)`, k, v); }
  /** "mono" writes its packs to INST_A and INST_B; any other group (the rebuilt install) to REST_A and REST_B. */
  buckets(): [R2Bucket, R2Bucket] { return this.group === "mono" ? [this.env.INST_A, this.env.INST_B] : [this.env.REST_A, this.env.REST_B]; }
  init(group: string): void {
    this.group = group;
    if (this.meta("last") === null) { this.setMeta("last", "0"); this.setMeta("lastDigest", ZERO); this.setMeta("writable", group === "mono" ? "1" : "0"); }
  }
  last(): { idx: number; digest: string } { return { idx: Number(this.meta("last")), digest: this.meta("lastDigest")! }; }

  apply(evs: Ev[]): void {
    for (const e of evs) {
      if (e.t === "ref") {
        if (e.oid) this.sql.exec(`INSERT OR REPLACE INTO refs VALUES (?, ?)`, e.ref, e.oid);
        else this.sql.exec(`DELETE FROM refs WHERE ref = ?`, e.ref);
      } else if (e.t === "task") this.sql.exec(`INSERT OR REPLACE INTO tasks VALUES (?, ?, ?)`, e.id, e.app, e.ref);
      else if (e.t === "pr") this.sql.exec(`INSERT OR REPLACE INTO prs VALUES (?, ?, ?, ?, ?, NULL)`, e.n, e.ref, e.app, e.session, e.head);
      else if (e.t === "merged") this.sql.exec(`UPDATE prs SET merged = ? WHERE n = ?`, e.commit, e.n);
      else if (e.t === "prov") this.sql.exec(`INSERT OR REPLACE INTO prov VALUES (?, ?, ?, ?)`, e.commit, e.ref, JSON.stringify(e.sources), e.session);
      else if (e.t === "pack") {
        this.sql.exec(`INSERT OR REPLACE INTO packs VALUES (?, ?, ?)`, e.id, e.checksum, e.bytes);
        for (const o of e.oids) this.sql.exec(`INSERT OR IGNORE INTO objs VALUES (?, ?)`, o, e.id);
      }
    }
  }
  /** Appends one entry, applies it, and writes a snapshot every SNAP_EVERY entries. */
  async commit(evs: Ev[], at: number): Promise<number> {
    const { idx, digest: prev } = this.last(), body = JSON.stringify(evs), i = idx + 1;
    const digest = await digestOf(prev, i, at, body);
    this.sql.exec(`INSERT INTO log VALUES (?, ?, ?, ?, ?)`, i, prev, digest, at, body);
    this.apply(evs);
    this.setMeta("last", String(i)); this.setMeta("lastDigest", digest);
    if (i % SNAP_EVERY === 0) await this.snapshot();
    return i;
  }
  foldRows(): Record<string, Row[]> {
    return Object.fromEntries(FOLD.map((t) => [t, this.sql.exec<Row>(`SELECT * FROM ${t} ORDER BY 1`).toArray()]));
  }
  /** SHA-256 of every fold row in key order: equal before the loss and after the restore means the same fold. */
  async foldDigest(group: string): Promise<{ digest: string; rows: Record<string, number>; last: number; lastDigest: string; writable: string | null }> {
    this.init(group);
    const rows = this.foldRows();
    return { digest: await sha256hex(JSON.stringify(rows)), rows: Object.fromEntries(Object.entries(rows).map(([t, r]) => [t, r.length])),
      last: this.last().idx, lastDigest: this.last().digest, writable: this.meta("writable") };
  }
  /** `R2Key::Snapshot`-style: `snapshot/<group>/core/0-<to>-<digest of entry to>`, create-only in both buckets. */
  async snapshot(): Promise<string> {
    const { idx, digest } = this.last();
    const key = `snapshot/mono/core/0-${idx}-${digest}`, body = JSON.stringify({ upto: idx, digest, rows: this.foldRows() });
    const sha = await sha256hex(body);
    await Promise.all(this.buckets().map((b) => b.put(key, body, { sha256: sha, onlyIf: new Headers({ "if-none-match": "*" }) })));
    this.sql.exec(`INSERT OR REPLACE INTO snaps VALUES (?, ?, ?)`, key, idx, digest);
    return key;
  }

  refGet(ref: string): string | null { return (this.sql.exec<Row>(`SELECT oid FROM refs WHERE ref = ?`, ref).toArray()[0]?.oid as string) ?? null; }
  has(oid: string): boolean { return this.sql.exec<Row>(`SELECT 1 FROM objs WHERE oid = ?`, oid).toArray().length > 0; }

  /** One command: validate, decide, store the pack in two buckets, then commit. Refused commands commit nothing. */
  async run(group: string, c: Cmd, placed: Set<string> = new Set()): Promise<{ ok: boolean; why?: string; idx?: number }> {
    this.init(group);
    if (this.meta("writable") !== "1") return { ok: false, why: "read-only until the restore opens writes" };
    const at = Date.now();
    if (c.k === "task") return { ok: true, idx: await this.commit([{ t: "task", id: c.id, app: c.app, ref: c.ref }], at) };
    if (c.k === "pr") {
      if (this.refGet(c.ref) !== c.head) return { ok: false, why: "head is not the ref's tip" };
      return { ok: true, idx: await this.commit([{ t: "pr", n: c.n, ref: c.ref, app: c.meta.app, session: c.meta.session, head: c.head }], at) };
    }
    if (this.refGet(c.ref) !== c.old) return { ok: false, why: `stale old for ${c.ref}` };
    // An `ActorKind::Agent` update of `refs/notes/ai` is outside its prefix (decisions/DECISIONS.md, "Agents do not
    // write `refs/notes/ai`"). Humans and Apps that are not `AgentApp`s may write notes.
    if (c.ref === "refs/notes/ai" && c.actor.t === "app" && AGENT_APPS.has(c.actor.app)) return { ok: false, why: "agent may not update refs/notes/ai" };
    if (c.ref.startsWith("refs/graftyard/")) {
      const task = c.ref.startsWith(SESSION_REFS) ? c.ref.slice(SESSION_REFS.length) : null;
      const ok = task && c.actor.t === "app" && c.actor.app === "graftyard-workspace" && c.actor.task === task
        && this.sql.exec<Row>(`SELECT 1 FROM tasks WHERE id = ?`, task).toArray().length > 0;
      if (!ok) return { ok: false, why: "host-owned ref" };
    }
    // validate: every object's SHA-1, every link present in the push or the store, then provenance from new commits.
    const mine = new Map(c.objects.map((o) => [o.oid, o]));
    for (const o of c.objects) if (!(await oidOk(o))) return { ok: false, why: `bad object ${o.oid}` };
    const commits: RawCommit[] = [], notes: NoteEntry[] = [];
    for (const o of c.objects) {
      const body = b64(o.data);
      const links = o.type === "commit" ? (({ tree, parents }) => [tree, ...parents])(commitHeader(body))
        : o.type === "tree" ? treeEntries(body).filter((e) => e.mode !== "160000").map((e) => e.oid) : [];
      for (const l of links) if (!mine.has(l) && !this.has(l)) return { ok: false, why: `missing ${l}` };
      if (o.type === "commit" && c.ref !== "refs/notes/ai" && !c.ref.startsWith("refs/graftyard/")) commits.push({ oid: o.oid, raw: body });
    }
    if (c.ref === "refs/notes/ai") {
      // The notes tree diff: blobs added under paths that name a commit (flat here, as `git notes add` writes them).
      const tip = commitHeader(b64(mine.get(c.new!)!.data));
      const tree = mine.get(tip.tree);
      if (tree) for (const e of treeEntries(b64(tree.data))) if (mine.has(e.oid)) notes.push({ path: e.name, mode: e.mode, type: "blob", oid: e.oid, bytes: null });
    }
    const { list } = provenanceOf(commits, notes);
    // decide: `Session` only for an App installation token whose AgentTask the trailer names, on that task's ref.
    const evs: Ev[] = [];
    for (const p of list) {
      let session: string | null = null;
      for (const s of p.sources) {
        if (s.t !== "Claimed" || s.key !== "GraftyardSession" || c.actor.t !== "app") continue;
        const id = sessionFromUrl(s.value, HOST, REPO_NAME);
        const task = id ? this.sql.exec<Row>(`SELECT app FROM tasks WHERE id = ?`, id).toArray()[0] : undefined;
        if (task && task.app === c.actor.app) session = id;
      }
      const sources: Source[] = session ? [...p.sources, { t: "Session", session }] : p.sources;
      const prior = this.sql.exec<Row>(`SELECT sources FROM prov WHERE commit_ = ?`, p.commit).toArray()[0];
      const merged = prior ? [...JSON.parse(prior.sources as string), ...sources] : sources;
      evs.push({ t: "prov", commit: p.commit, ref: c.ref === "refs/notes/ai" ? (this.sql.exec<Row>(`SELECT ref FROM prov WHERE commit_ = ?`, p.commit).toArray()[0]?.ref as string) ?? null : c.ref,
        sources: merged, session: session ?? ((this.sql.exec<Row>(`SELECT session FROM prov WHERE commit_ = ?`, p.commit).toArray()[0]?.session as string) ?? null) });
    }
    if (c.objects.length) {
      const body = JSON.stringify(c.objects), checksum = await sha256hex(body), id = checksum.slice(0, 32);
      if (!placed.has(id)) await Promise.all(this.buckets().map((b) => b.put(`pack/mono/${id}`, body, { sha256: checksum })));
      evs.unshift({ t: "pack", id, checksum, bytes: body.length, oids: c.objects.map((o) => o.oid) });
    }
    evs.push({ t: "ref", ref: c.ref, oid: c.new });
    if (c.k === "merge") evs.push({ t: "merged", n: c.n!, commit: c.new! });
    return { ok: true, idx: await this.commit(evs, at) };
  }

  /** A batch of commands, run in order. Their packs are placed in both buckets first, in parallel: keys are
   *  content-addressed, so a pack of a command that is then refused is an unreferenced object for GC, as a push whose
   *  `decide` refuses after `PackStore::place` leaves one. */
  async runBatch(group: string, cmds: Cmd[]): Promise<{ ok: boolean; why?: string; idx?: number }[]> {
    this.init(group);
    const placed = new Set<string>();
    await Promise.all(cmds.map(async (c) => {
      if ((c.k !== "push" && c.k !== "merge") || !c.objects.length) return;
      const body = JSON.stringify(c.objects), checksum = await sha256hex(body), id = checksum.slice(0, 32);
      await Promise.all(this.buckets().map((b) => b.put(`pack/mono/${id}`, body, { sha256: checksum })));
      placed.add(id);
    }));
    const out = [];
    for (const c of cmds) out.push(await this.run(group, c, placed));
    return out;
  }

  /** Journey A6 for each commit: `session:<id>` when the fold links a session whose host-owned ref and stored log read
   *  back from the buckets, `pr:<n>` through `Merged` (a squash) or the PR on the commit's ref, else `none`. */
  async resolve(group: string, commits: string[]): Promise<Record<string, string>> {
    this.init(group);
    const out: Record<string, string> = {}, logOk = new Map<string, boolean>();
    for (const c of commits) {
      const merged = this.sql.exec<Row>(`SELECT n FROM prs WHERE merged = ?`, c).toArray()[0];
      if (merged) { out[c] = `pr:${merged.n}`; continue; }
      const p = this.sql.exec<Row>(`SELECT ref, session FROM prov WHERE commit_ = ?`, c).toArray()[0];
      if (p?.session) {
        const s = p.session as string;
        if (!logOk.has(s)) logOk.set(s, await this.sessionLogReads(s));
        if (logOk.get(s)) { out[c] = `session:${s}`; continue; }
      }
      const pr = p?.ref ? this.sql.exec<Row>(`SELECT n FROM prs WHERE ref = ? ORDER BY n DESC LIMIT 1`, p.ref).toArray()[0] : undefined;
      out[c] = pr ? `pr:${pr.n}` : "none";
    }
    return out;
  }
  /** The session's host-owned ref names a commit whose `log.json` blob is in a stored pack that reads back. */
  async sessionLogReads(id: string): Promise<boolean> {
    const tip = this.refGet(`${SESSION_REFS}${id}`);
    if (!tip) return false;
    const obj = async (oid: string): Promise<RawObj | null> => {
      const pack = this.sql.exec<Row>(`SELECT pack FROM objs WHERE oid = ?`, oid).toArray()[0]?.pack as string | undefined;
      if (!pack) return null;
      const r = await this.buckets()[0].get(`pack/mono/${pack}`);
      return r ? ((await r.json()) as RawObj[]).find((o) => o.oid === oid) ?? null : null;
    };
    const c = await obj(tip);
    const tree = c && (await obj(commitHeader(b64(c.data)).tree));
    const log = tree && treeEntries(b64(tree.data)).find((e) => e.name === "log.json");
    const blob = log && (await obj(log.oid));
    return !!blob && JSON.parse(new TextDecoder().decode(b64(blob.data))).id === id;
  }

  // ---- export, read by the vault (design/DESIGN.md, Install operations, Vault) ----
  exportEntries(group: string, from: number, max: number): Entry[] {
    this.init(group);
    return this.sql.exec<Entry & Row>(`SELECT * FROM log WHERE idx >= ? ORDER BY idx LIMIT ?`, from, max).toArray() as Entry[];
  }
  async exportObject(group: string, key: string): Promise<string | null> {
    this.init(group);
    return (await (await this.buckets()[0].get(key))?.text()) ?? null;
  }
  snapshotKeys(group: string): string[] { this.init(group); return this.sql.exec<Row>(`SELECT key FROM snaps ORDER BY upto`).toArray().map((r) => r.key as string); }

  // ---- the loss: a destructive deploy that deletes the group and empties its buckets ----
  async wipe(group: string): Promise<object> {
    this.init(group);
    let deleted = 0;
    for (const b of this.buckets()) for (;;) {
      const page = await b.list({ limit: 1000 });
      if (!page.objects.length) break;
      await b.delete(page.objects.map((o) => o.key));
      deleted += page.objects.length;
    }
    await this.ctx.storage.deleteAll();
    this.schema();
    return { deleted, entries: this.last().idx };
  }

  // ---- whole-install restore steps 2, 3, and 5, driven by the vault ----
  /** Step 2: the newest vault snapshot at or before the cutoff. Its digest must be the vault log's entry `upto`. */
  async restoreSnapshot(group: string, snap: { upto: number; digest: string; rows: Record<string, Row[]> }): Promise<object> {
    this.init(group);
    if (this.last().idx !== 0) return { ok: false, why: "group not empty" };
    for (const [t, rows] of Object.entries(snap.rows)) for (const r of rows) {
      const cols = Object.keys(r);
      this.sql.exec(`INSERT INTO ${t} (${cols.join(", ")}) VALUES (${cols.map(() => "?").join(", ")})`, ...cols.map((c) => r[c]));
    }
    this.setMeta("last", String(snap.upto)); this.setMeta("lastDigest", snap.digest); this.setMeta("writable", "0");
    return { ok: true, upto: snap.upto };
  }
  /** Step 2: the log tail. Each entry must follow the last one, chain its `prev`, and hash to its `digest`. */
  async restoreTail(group: string, entries: Entry[]): Promise<object> {
    this.init(group);
    for (const e of entries) {
      const { idx, digest } = this.last();
      if (e.idx !== idx + 1 || e.prev !== digest || (await digestOf(e.prev, e.idx, e.at, e.body)) !== e.digest) return { ok: false, why: `chain breaks at ${e.idx}` };
      this.sql.exec(`INSERT INTO log VALUES (?, ?, ?, ?, ?)`, e.idx, e.prev, e.digest, e.at, e.body);
      this.apply(JSON.parse(e.body));
      this.setMeta("last", String(e.idx)); this.setMeta("lastDigest", e.digest);
    }
    return { ok: true, last: this.last().idx };
  }
  restorePackList(group: string, offset: number, max: number): { id: string; checksum: string }[] {
    this.init(group);
    return this.sql.exec<Row>(`SELECT id, checksum FROM packs ORDER BY id LIMIT ? OFFSET ?`, max, offset).toArray() as { id: string; checksum: string }[];
  }
  /** Step 3: one referenced pack into both of this install's buckets, refused unless its SHA-256 is `PackRef.checksum`. */
  async restorePack(group: string, id: string, body: string): Promise<boolean> {
    this.init(group);
    const want = this.sql.exec<Row>(`SELECT checksum FROM packs WHERE id = ?`, id).toArray()[0]?.checksum;
    const sha = await sha256hex(body);
    if (!want || sha !== want) return false;
    await Promise.all(this.buckets().map((b) => b.put(`pack/mono/${id}`, body, { sha256: sha })));
    this.sql.exec(`INSERT OR IGNORE INTO copied VALUES (?)`, id);
    return true;
  }
  /** Step 5: writes open only when the index is the cutoff's and every referenced pack is copied; else read-only. */
  restoreOpen(group: string, cutoff: number): object {
    this.init(group);
    const packs = this.sql.exec<Row>(`SELECT COUNT(*) AS n FROM packs`).one().n as number;
    const missing = this.sql.exec<Row>(`SELECT COUNT(*) AS n FROM packs WHERE id NOT IN (SELECT id FROM copied)`).one().n as number;
    const idx = this.last().idx, ok = idx === cutoff && missing === 0;
    if (ok) this.setMeta("writable", "1");
    return { ok, idx, cutoff, packs, missing };
  }
}

const json = (v: unknown, status = 200) => Response.json(v, { status });

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url), auth = req.headers.get("authorization");
    const harness = auth === `Bearer ${env.M0_TOKEN}`, exporter = auth === `Bearer ${env.EXPORT_TOKEN}`;
    // The export token reads `/export/*` only; every other route takes the harness (operator) token.
    if (!(harness || (exporter && url.pathname.startsWith("/export/")))) return new Response("unauthorized", { status: 401 });
    const body = req.method === "POST" ? await req.json<Record<string, any>>() : {};
    const group: string = body.group ?? "mono", stub = env.REPO.get(env.REPO.idFromName(group));
    try {
      switch (url.pathname) {
        case "/run": return json(await stub.runBatch(group, body.cmds as Cmd[]));
        case "/resolve": return json(await stub.resolve(group, body.commits));
        case "/digest": return json(await stub.foldDigest(group));
        case "/wipe": return json(await stub.wipe(group));
        case "/export/entries": return json(await stub.exportEntries(group, body.from, body.max));
        case "/export/object": return new Response(await stub.exportObject(group, body.key), { status: 200 });
        case "/export/snapshots": return json(await stub.snapshotKeys(group));
        case "/restore/snapshot": return json(await stub.restoreSnapshot(group, body.snap));
        case "/restore/tail": return json(await stub.restoreTail(group, body.entries));
        case "/restore/packlist": return json(await stub.restorePackList(group, body.offset, body.max));
        case "/restore/pack": return json({ ok: await stub.restorePack(group, body.id, body.body) });
        case "/restore/open": return json(await stub.restoreOpen(group, body.cutoff));
      }
    } catch (e) { return json({ error: String(e) }, 500); }
    return json({ error: "not found" }, 404);
  },
} satisfies ExportedHandler<Env>;
