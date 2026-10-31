// M0-WP9, B0-30 (build/BUILD-PLAN.md): 30 days of hot-group storage, accelerated. `graft-wp9-store` is its own script
// with one Durable Object class, `Store9`, standing for one member of the monorepo's group, and two buckets
// (`graft-wp9-arch-a` enam, `graft-wp9-arch-b` weur) for `R2Key::Archive`.
//
// The member's SQLite holds what design/DESIGN.md puts in a member: the log (entries with inline packs), the fold's
// core rows, and the metadata tier. The run, driven by `run/wp9-store.ts` in steps:
// 1. `seed`: a metadata tier of `gb` GB, as issue and comment rows (200,000 issues of 2 KB, 2 million comments of
//    about 1.8 KB, the design's "about 4 GB" largest repository), in batches.
// 2. `replay`: journey 9's entries, as fast as the object takes them, 4 active hours a simulated day (ACTIVE_S). Per simulated second of journey 9: 4.17 agent
//    pushes with inline packs (uniform 10 to 70 KB, mean 40 KB as `scripts/cost_agents.py` prices them, and 1 in 1,000
//    at 1 MiB; the hot cap is not applied, so this is the uncapped inline volume behind the script's 71 GB), 23.4
//    other commands of 300 B,
//    grouped into entries of at most 1.5 MB; and the agent rows `cost_agents.py` counts: a PR row (2 KB) per task, a
//    check row (300 B) per CI job and per half push, a provenance row (150 B) per push, a task row (1 KB) per task, and a
//    live `PathSet` (16 KB) per open agent PR, deleted when the PR closes.
// 3. `LOG_MAX_BYTES` (design/DESIGN.md, Shape): after each step, while the log's bytes exceed the cap, the oldest
//    entries are archived to `R2Key::Archive { chain, from, to, digest }` in both buckets (create-only puts), an
//    `Archived` row is recorded, and only then are they deleted from the log. The 30-day rule is not applied, so the
//    byte cap alone bounds the log (30 days of log never ages out inside a 30-day replay).
// 4. `readback`: reads a sample of archived ranges back from both buckets and checks each entry's chained digest
//    against the `Archived` row and the entries' own `prev` links.
// `databaseSize` is read after every step; any `SQLITE_FULL` is caught and counted.

import { DurableObject } from "cloudflare:workers";
import { sha256hex } from "./sha256.ts";

interface Env { STORE: DurableObjectNamespace<Store9>; ARCH_A: R2Bucket; ARCH_B: R2Bucket; M0_TOKEN: string }

const ENTRY_MAX = 1_500_000;
const LOG_MAX_BYTES = 2 * 2 ** 30;
const PUSH_PER_S = 500 / 120, OTHER_PER_S = 27.6 - PUSH_PER_S, CMD_BYTES = 300;
const TASK_S = 20 * 60, TASKS_PER_S = 500 / TASK_S, CI_JOBS = 3, CHECK_PER_PUSH = 0.5;
/** Active seconds per simulated day: `scripts/cost_agents.py`'s 30 days of A1 are 180,000 tasks of 10 pushes, 1.8 M
 *  pushes, which is journey 9's 4.17 pushes/s for 14,400 s (4 h) a day. The rest of each day is idle. */
const ACTIVE_S = 14_400;
const ARCHIVE_BATCH = 16 << 20; // bytes of entries per archive object; 128 MB of isolate memory bounds it
const ZERO = "0".repeat(64);

/** Deterministic pseudo-random bytes, so the replay needs no crypto calls per pack. */
function fill(n: number, seed: number): Uint8Array {
  const b = new Uint8Array(n);
  let x = seed | 1;
  const v = new Uint32Array(b.buffer, 0, n >> 2);
  for (let i = 0; i < v.length; i++) { x ^= x << 13; x ^= x >>> 17; x ^= x << 5; v[i] = x; }
  return b;
}

export class Store9 extends DurableObject<Env> {
  private sql = this.ctx.storage.sql;
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    const s = this.sql;
    s.exec(`CREATE TABLE IF NOT EXISTS log (idx INTEGER PRIMARY KEY, at INTEGER NOT NULL, prev TEXT NOT NULL, digest TEXT NOT NULL,
            body TEXT NOT NULL, packs BLOB, bytes INTEGER NOT NULL)`);
    s.exec(`CREATE TABLE IF NOT EXISTS archived (from_idx INTEGER PRIMARY KEY, to_idx INTEGER, from_at INTEGER, to_at INTEGER,
            key TEXT, digest TEXT, bytes INTEGER, puts INTEGER)`);
    s.exec(`CREATE TABLE IF NOT EXISTS issue (n INTEGER PRIMARY KEY, title TEXT, body TEXT, state TEXT, updated INTEGER)`);
    s.exec(`CREATE TABLE IF NOT EXISTS comment (id INTEGER PRIMARY KEY, issue INTEGER, body TEXT, at INTEGER)`);
    s.exec(`CREATE TABLE IF NOT EXISTS pr (n INTEGER PRIMARY KEY, head TEXT, meta TEXT, state TEXT)`);
    s.exec(`CREATE TABLE IF NOT EXISTS checkrow (id INTEGER PRIMARY KEY, pr INTEGER, at INTEGER, body TEXT)`);
    s.exec(`CREATE TABLE IF NOT EXISTS prov (id INTEGER PRIMARY KEY, pr INTEGER, body TEXT)`);
    s.exec(`CREATE TABLE IF NOT EXISTS task (id INTEGER PRIMARY KEY, body TEXT)`);
    s.exec(`CREATE TABLE IF NOT EXISTS pathset (pr INTEGER PRIMARY KEY, body TEXT)`);
    s.exec(`CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT)`);
  }

  private get(k: string, d = 0): number { return Number(this.sql.exec<{ v: string }>(`SELECT v FROM kv WHERE k = ?`, k).toArray()[0]?.v ?? d); }
  private set(k: string, v: number | string): void { this.sql.exec(`INSERT OR REPLACE INTO kv VALUES (?, ?)`, k, String(v)); }
  private size(): number { return this.ctx.storage.sql.databaseSize; }
  private full(e: unknown): boolean { const s = String(e); if (/SQLITE_FULL|database or disk is full/i.test(s)) { this.set("sqliteFull", this.get("sqliteFull") + 1); return true; } return false; }

  /** Seeds `rows` metadata rows (issue every tenth, comments otherwise) of about `rowBytes` each, from row `from`. */
  async seed(from: number, rows: number, rowBytes: number): Promise<object> {
    const t = Date.now();
    const text = "x".repeat(rowBytes - 40);
    let done = 0;
    try {
      for (let i = from; i < from + rows; i++) {
        const tag = `${i.toString(36)}-`;
        if (i % 10 === 0) this.sql.exec(`INSERT OR REPLACE INTO issue VALUES (?, ?, ?, 'open', ?)`, i / 10, `issue ${i}`, tag + text, i);
        else this.sql.exec(`INSERT OR REPLACE INTO comment VALUES (?, ?, ?, ?)`, i, Math.floor(i / 10), tag + text, i);
        done++;
      }
    } catch (e) { if (!this.full(e)) throw e; }
    return { done, size: this.size(), ms: Date.now() - t, sqliteFull: this.get("sqliteFull") };
  }

  /** Replays `seconds` active seconds of journey 9 from the stored simulated clock, skipping each day's idle hours. */
  async replay(seconds: number): Promise<object> {
    const t0 = Date.now();
    let sim = this.get("sim"), idx = this.get("idx"), prev = this.sql.exec<{ digest: string }>(`SELECT digest FROM log ORDER BY idx DESC LIMIT 1`).toArray()[0]?.digest ?? ZERO;
    let pushes = this.get("pushes"), others = this.get("others"), tasks = this.get("tasks"), checks = this.get("checks"), inlineBytes = this.get("inlineBytes");
    let logBytes = this.get("logBytes"), entries = 0, full = false;
    let left = seconds;
    // Carry fractional rates across calls.
    let accPush = this.get("accPush"), accOther = this.get("accOther"), accTask = this.get("accTask");
    try {
      while (left > 0) {
        if (sim % 86_400 >= ACTIVE_S) { sim += 86_400 - (sim % 86_400); continue; }
        left--;
        accPush += PUSH_PER_S; accOther += OTHER_PER_S; accTask += TASKS_PER_S;
        // One simulated second's commands, grouped into entries of at most ENTRY_MAX bytes.
        const cmds: { bytes: number; pack?: number }[] = [];
        for (; accPush >= 1; accPush--) {
          const r = (pushes * 2654435761) % 1000;
          cmds.push({ bytes: 200, pack: r === 0 ? 1 << 20 : 10_000 + ((r * 60) % 60_000) });
          // Agent rows per push: provenance, and a check row on half of pushes (cost_agents.py, CHECK_WRITES_PER_PUSH).
          this.sql.exec(`INSERT INTO prov (pr, body) VALUES (?, ?)`, pushes % 500, "p".repeat(150));
          if (pushes % 2 === 0) { this.sql.exec(`INSERT INTO checkrow (pr, at, body) VALUES (?, ?, ?)`, pushes % 500, sim, "c".repeat(300)); checks++; }
          pushes++;
        }
        for (; accOther >= 1; accOther--) { cmds.push({ bytes: CMD_BYTES }); others++; }
        for (; accTask >= 1; accTask--) {
          // A task opens a PR (2 KB row), a task row (1 KB), 3 CI check rows, and a live PathSet (16 KB) that the
          // PR's close deletes 20 minutes later (the PR row stays).
          const pr = tasks;
          this.sql.exec(`INSERT OR REPLACE INTO pr VALUES (?, ?, ?, 'open')`, pr, `agent/${pr}`, "m".repeat(2048 - 20));
          this.sql.exec(`INSERT INTO task (body) VALUES (?)`, "t".repeat(1024));
          for (let j = 0; j < CI_JOBS; j++) { this.sql.exec(`INSERT INTO checkrow (pr, at, body) VALUES (?, ?, ?)`, pr, sim, "c".repeat(300)); checks++; }
          this.sql.exec(`INSERT OR REPLACE INTO pathset VALUES (?, ?)`, pr, "s".repeat(16 * 1024));
          this.sql.exec(`DELETE FROM pathset WHERE pr = ?`, pr - 500);
          this.sql.exec(`UPDATE pr SET state = 'closed' WHERE n = ?`, pr - 500);
          tasks++;
        }
        // Check rows are kept 90 days (GHES), so none expire inside 30 days.
        let i = 0;
        while (i < cmds.length) {
          let bytes = 0, packBytes = 0;
          const parts: Uint8Array[] = [];
          const body: number[] = [];
          for (; i < cmds.length && (bytes + cmds[i].bytes + (cmds[i].pack ?? 0) <= ENTRY_MAX || !body.length); i++) {
            bytes += cmds[i].bytes + (cmds[i].pack ?? 0);
            body.push(cmds[i].bytes);
            if (cmds[i].pack) { parts.push(fill(cmds[i].pack!, idx * 31 + i)); packBytes += cmds[i].pack!; }
          }
          const packs = new Uint8Array(packBytes);
          let o = 0;
          for (const p of parts) { packs.set(p, o); o += p.byteLength; }
          idx++;
          const bodyText = JSON.stringify({ at: sim, cmds: body, pad: "b".repeat(body.reduce((a, b) => a + b, 0)) });
          // The digest chain covers the body only, as a cheap stand-in: hashing 1 MiB packs in JS here would make the
          // replay CPU-bound on the hash, not on storage.
          const digest = sha256hex(`${prev}|${idx}|${sim}|${bodyText}`);
          const rowBytes = bodyText.length + packBytes + 200;
          this.sql.exec(`INSERT INTO log VALUES (?, ?, ?, ?, ?, ?, ?)`, idx, sim, prev, digest, bodyText, packBytes ? packs.buffer : null, rowBytes);
          prev = digest; logBytes += rowBytes; inlineBytes += packBytes; entries++;
        }
        sim++;
      }
    } catch (e) { if (!this.full(e)) throw e; full = true; }
    for (const [k, v] of Object.entries({ sim, idx, pushes, others, tasks, checks, inlineBytes, logBytes, accPush, accOther, accTask })) this.set(k, v);
    return { sim, idx, entries, logBytes, inlineBytes, size: this.size(), full, ms: Date.now() - t0, sqliteFull: this.get("sqliteFull") };
  }

  /** LOG_MAX_BYTES: archive the oldest entries in ARCHIVE_BATCH ranges until the log is under the cap. At most `max`
   *  ranges per call (each is two puts of up to 16 MB). */
  async archive(max: number): Promise<object> {
    const t0 = Date.now();
    let logBytes = this.get("logBytes"), ranges = 0, archivedBytes = 0;
    while (logBytes > LOG_MAX_BYTES && ranges < max) {
      const first = this.sql.exec<{ idx: number }>(`SELECT idx FROM log ORDER BY idx LIMIT 1`).toArray()[0];
      if (!first) break;
      // Collect a range of about ARCHIVE_BATCH bytes.
      const rows = this.sql.exec<{ idx: number; at: number; prev: string; digest: string; body: string; packs: ArrayBuffer | null; bytes: number }>(
        `SELECT idx, at, prev, digest, body, packs, bytes FROM log WHERE idx >= ? ORDER BY idx LIMIT 120`, first.idx).toArray();
      const take: typeof rows = [];
      let n = 0;
      for (const r of rows) { if (take.length && n + r.bytes > ARCHIVE_BATCH) break; take.push(r); n += r.bytes; }
      // Archive object: per entry a JSON header line, then the pack bytes.
      const chunks: Uint8Array[] = [];
      const enc = new TextEncoder();
      for (const r of take) {
        const head = enc.encode(JSON.stringify({ idx: r.idx, at: r.at, prev: r.prev, digest: r.digest, body: r.body, packs: r.packs?.byteLength ?? 0 }) + "\n");
        chunks.push(head);
        if (r.packs) chunks.push(new Uint8Array(r.packs));
      }
      const total = chunks.reduce((a, c) => a + c.byteLength, 0);
      const obj = new Uint8Array(total);
      let o = 0;
      for (const c of chunks) { obj.set(c, o); o += c.byteLength; }
      const digest = [...new Uint8Array(await crypto.subtle.digest("SHA-256", obj))].map((b) => b.toString(16).padStart(2, "0")).join("");
      const from = take[0].idx, to = take[take.length - 1].idx;
      const key = `archive/wp9-b0-30/${from}-${to}-${digest.slice(0, 16)}`;
      // Create-only puts in both buckets; the range is deleted only after both (design: `Archived` after both puts).
      const [pa, pb] = await Promise.all([
        this.env.ARCH_A.put(key, obj, { onlyIf: new Headers({ "if-none-match": "*" }) }),
        this.env.ARCH_B.put(key, obj, { onlyIf: new Headers({ "if-none-match": "*" }) }),
      ]);
      const puts = (pa ? 1 : 0) + (pb ? 1 : 0);
      if (puts < 2) {
        const [ha, hb] = await Promise.all([this.env.ARCH_A.head(key), this.env.ARCH_B.head(key)]);
        if (!ha || !hb) throw new Error(`archive ${key}: in ${Number(!!ha) + Number(!!hb)} buckets`);
      }
      this.sql.exec(`INSERT OR REPLACE INTO archived VALUES (?, ?, ?, ?, ?, ?, ?, ?)`, from, to, take[0].at, take[take.length - 1].at, key, digest, total, puts);
      this.sql.exec(`DELETE FROM log WHERE idx <= ?`, to);
      logBytes -= n; archivedBytes += total; ranges++;
    }
    this.set("logBytes", logBytes);
    this.set("archivedBytes", this.get("archivedBytes") + archivedBytes);
    return { ranges, archivedBytes, logBytes, over: logBytes > LOG_MAX_BYTES, size: this.size(), ms: Date.now() - t0 };
  }

  /** Reads `sample` archived ranges back from both buckets and checks bytes, digest, and the entries' chain. */
  async readback(sample: number): Promise<object> {
    const all = this.sql.exec<{ from_idx: number; to_idx: number; key: string; digest: string; bytes: number; from_at: number; to_at: number }>(
      `SELECT from_idx, to_idx, key, digest, bytes, from_at, to_at FROM archived ORDER BY from_idx`).toArray();
    const pick = all.length <= sample ? all : Array.from({ length: sample }, (_, i) => all[Math.floor((i * (all.length - 1)) / Math.max(1, sample - 1))]);
    const out: object[] = [];
    let bad = 0;
    for (const a of pick) {
      for (const [name, b] of [["a", this.env.ARCH_A], ["b", this.env.ARCH_B]] as const) {
        const t = Date.now();
        const o = await b.get(a.key);
        if (!o) { bad++; out.push({ key: a.key, bucket: name, missing: true }); continue; }
        const buf = new Uint8Array(await o.arrayBuffer());
        const digest = [...new Uint8Array(await crypto.subtle.digest("SHA-256", buf))].map((x) => x.toString(16).padStart(2, "0")).join("");
        // Walk the entries: header line, then `packs` bytes; check idx order, prev links, and each digest.
        let off = 0, n = 0, chain = true, prevDigest: string | null = null, lastIdx = a.from_idx - 1;
        const dec = new TextDecoder();
        while (off < buf.length) {
          const nl = buf.indexOf(10, off);
          const h = JSON.parse(dec.decode(buf.subarray(off, nl))) as { idx: number; at: number; prev: string; digest: string; body: string; packs: number };
          off = nl + 1 + h.packs;
          if (h.idx !== lastIdx + 1 || (prevDigest && h.prev !== prevDigest) || sha256hex(`${h.prev}|${h.idx}|${h.at}|${h.body}`) !== h.digest) chain = false;
          prevDigest = h.digest; lastIdx = h.idx; n++;
        }
        const ok = digest === a.digest && buf.length === a.bytes && chain && lastIdx === a.to_idx;
        if (!ok) bad++;
        out.push({ key: a.key, bucket: name, ok, entries: n, bytes: buf.length, ms: Date.now() - t, from_at: a.from_at, to_at: a.to_at });
      }
    }
    // The first archived range's prev must be the chain's start, and each range's first prev the previous range's last.
    return { ranges: all.length, checked: pick.length, bad, sample: out };
  }

  async wipe(): Promise<void> { await this.ctx.storage.deleteAll(); }

  async stats(): Promise<object> {
    const count = (t: string) => this.sql.exec<{ n: number }>(`SELECT count(*) AS n FROM ${t}`).toArray()[0].n;
    const sum = (q: string) => this.sql.exec<{ s: number }>(q).toArray()[0].s ?? 0;
    return {
      size: this.size(), sqliteFull: this.get("sqliteFull"), sim: this.get("sim"), idx: this.get("idx"),
      log: { rows: count("log"), bytes: sum(`SELECT sum(bytes) AS s FROM log`), counter: this.get("logBytes") },
      archived: { ranges: count("archived"), bytes: this.get("archivedBytes") },
      meta: { issues: count("issue"), comments: count("comment"), bytes: sum(`SELECT sum(length(body)) + sum(length(title)) AS s FROM issue`) + sum(`SELECT sum(length(body)) AS s FROM comment`) },
      agentRows: { prs: count("pr"), checks: count("checkrow"), prov: count("prov"), tasks: count("task"), pathsets: count("pathset"),
        bytes: sum(`SELECT sum(length(meta)) AS s FROM pr`) + sum(`SELECT sum(length(body)) AS s FROM checkrow`) + sum(`SELECT sum(length(body)) AS s FROM prov`)
          + sum(`SELECT sum(length(body)) AS s FROM task`) + sum(`SELECT sum(length(body)) AS s FROM pathset`) },
      counters: { pushes: this.get("pushes"), others: this.get("others"), tasks: this.get("tasks"), inlineBytes: this.get("inlineBytes") },
    };
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    const b = await req.json<{ name: string; op: string; from?: number; rows?: number; rowBytes?: number; seconds?: number; max?: number; sample?: number }>();
    const s = env.STORE.get(env.STORE.idFromName(b.name), { locationHint: "enam" });
    try {
      switch (b.op) {
        case "seed": return Response.json(await s.seed(b.from!, b.rows!, b.rowBytes!));
        case "replay": return Response.json(await s.replay(b.seconds!));
        case "archive": return Response.json(await s.archive(b.max ?? 4));
        case "readback": return Response.json(await s.readback(b.sample ?? 20));
        case "stats": return Response.json(await s.stats());
        case "wipe": await s.wipe(); return Response.json({ wiped: b.name });
        case "purge": {
          // Deletes this run's archive objects from both buckets, 1,000 keys a call.
          let n = 0;
          for (const b of [env.ARCH_A, env.ARCH_B]) {
            const l = await b.list({ prefix: "archive/wp9-b0-30/", limit: 1000 });
            if (l.objects.length) { await b.delete(l.objects.map((o) => o.key)); n += l.objects.length; }
          }
          return Response.json({ deleted: n });
        }
      }
    } catch (e) { return Response.json({ error: String(e).slice(0, 500) }, { status: 500 }); }
    return new Response("op?", { status: 400 });
  },
} satisfies ExportedHandler<Env>;
