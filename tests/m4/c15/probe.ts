// C15 probe (decisions/OPEN-QUESTIONS.md): does FTS5 work inside Durable Object SQLite, in the shapes `SearchShard`
// needs? Its own throwaway script, never a site script. One `Fts` object per (jurisdiction, name): `/run?j=eu&n=x`.
// It reports the SQLite version and compile options, then runs each check and reports pass or the error text.

import { DurableObject } from "cloudflare:workers";

type Env = { FTS: DurableObjectNamespace<Fts>; TOKEN: string };

type Check = { name: string; ok: boolean; detail: unknown };

const WORDS = "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima mike november oscar papa".split(" ");

function doc(i: number): string {
  // Deterministic word text: doc i holds WORDS[i % 16] and WORDS[(i * 7) % 16] among 40 words.
  const out: string[] = [];
  for (let k = 0; k < 40; k++) out.push(WORDS[(i * (k + 1) + k) % 16]);
  out.push(WORDS[i % 16], WORDS[(i * 7) % 16], `doc${i}`);
  return out.join(" ");
}

export class Fts extends DurableObject<Env> {
  run(n: number): { sqlite: string; options: string[]; colo: string | null; checks: Check[]; dbBytes: number } {
    const sql = this.ctx.storage.sql;
    const checks: Check[] = [];
    const check = (name: string, f: () => unknown) => {
      try {
        checks.push({ name, ok: true, detail: f() });
      } catch (e) {
        checks.push({ name, ok: false, detail: String(e) });
      }
    };
    const rows = (q: string, ...b: unknown[]) => sql.exec(q, ...b).toArray();

    let sqlite = "";
    let options: string[] = [];
    check("version", () => (sqlite = String(rows("SELECT sqlite_version() AS v")[0].v)));
    check("compile_options", () => (options = rows("PRAGMA compile_options").map((r) => String(Object.values(r)[0]))));

    // 1. The metadata tier's shape (workers/site/src/tier.rs): contentless with contentless_delete.
    check("create_contentless_delete", () => {
      sql.exec("DROP TABLE IF EXISTS c_fts");
      sql.exec("CREATE VIRTUAL TABLE c_fts USING fts5(title, body, content='', contentless_delete=1)");
      return "created";
    });
    check("insert_match_delete_contentless", () => {
      sql.exec("INSERT INTO c_fts (rowid, title, body) VALUES (1, 'merge queue stalls', 'the queue never lands')");
      sql.exec("INSERT INTO c_fts (rowid, title, body) VALUES (2, 'webhook retries', 'deliveries time out at ten seconds')");
      const before = rows("SELECT rowid FROM c_fts WHERE c_fts MATCH 'queue' ORDER BY rowid").map((r) => r.rowid);
      sql.exec("DELETE FROM c_fts WHERE rowid = 1");
      const after = rows("SELECT rowid FROM c_fts WHERE c_fts MATCH 'queue'").map((r) => r.rowid);
      if (JSON.stringify(before) !== "[1]" || after.length !== 0) throw new Error(`before ${before} after ${after}`);
      return { before, after };
    });

    // 2. A search shard's shape: content-bearing, porter + unicode61, bm25 ranking, snippet, column filter, prefix.
    check("create_content_porter", () => {
      sql.exec("DROP TABLE IF EXISTS s_fts");
      sql.exec(
        "CREATE VIRTUAL TABLE s_fts USING fts5(repo UNINDEXED, number UNINDEXED, title, body, tokenize='porter unicode61 remove_diacritics 2')",
      );
      return "created";
    });
    check(`insert_${n}_docs`, () => {
      const t0 = Date.now();
      for (let i = 0; i < n; i++) sql.exec("INSERT INTO s_fts (repo, number, title, body) VALUES (?, ?, ?, ?)", i % 50, i, `title ${WORDS[i % 16]}`, doc(i));
      return { ms: Date.now() - t0 };
    });
    check("match_bm25_snippet", () => {
      const t0 = Date.now();
      const r = rows(
        "SELECT number, bm25(s_fts) AS score, snippet(s_fts, 3, '[', ']', '…', 6) AS snip FROM s_fts WHERE s_fts MATCH ? ORDER BY bm25(s_fts) LIMIT 5",
        "doc7",
      );
      if (r.length !== 1 || r[0].number !== 7 || !String(r[0].snip).includes("[doc7]")) throw new Error(JSON.stringify(r));
      return { ms: Date.now() - t0, r };
    });
    check("count_term_column_prefix_phrase", () => {
      const t0 = Date.now();
      const term = Number(rows("SELECT count(*) AS c FROM s_fts WHERE s_fts MATCH 'title:hotel'")[0].c);
      // title is `title WORDS[i % 16]`: hotel is index 7, so i % 16 == 7.
      let want = 0;
      for (let i = 0; i < n; i++) if (i % 16 === 7) want++;
      const prefix = Number(rows("SELECT count(*) AS c FROM s_fts WHERE s_fts MATCH 'doc12*'")[0].c);
      let wantPrefix = 0;
      for (let i = 0; i < n; i++) if (`doc${i}`.startsWith("doc12")) wantPrefix++;
      const stem = Number(rows("SELECT count(*) AS c FROM s_fts WHERE s_fts MATCH 'title:titles'")[0].c);
      if (term !== want || prefix !== wantPrefix || stem !== n) throw new Error(JSON.stringify({ term, want, prefix, wantPrefix, stem, n }));
      return { ms: Date.now() - t0, term, prefix, stem };
    });
    check("filter_unindexed_and_rank_order", () => {
      const r = rows("SELECT number FROM s_fts WHERE s_fts MATCH 'alpha AND bravo' AND repo = 3 ORDER BY rank LIMIT 3");
      return { n: r.length, first: r.map((x) => x.number) };
    });
    check("trigram_tokenizer", () => {
      sql.exec("DROP TABLE IF EXISTS t_fts");
      sql.exec("CREATE VIRTUAL TABLE t_fts USING fts5(body, tokenize='trigram')");
      sql.exec("INSERT INTO t_fts (body) VALUES ('graftyard search shard')");
      const c = Number(rows("SELECT count(*) AS c FROM t_fts WHERE t_fts MATCH 'tyar'")[0].c);
      if (c !== 1) throw new Error(`count ${c}`);
      return c;
    });
    check("optimize_and_integrity", () => {
      sql.exec("INSERT INTO s_fts (s_fts) VALUES ('optimize')");
      sql.exec("INSERT INTO s_fts (s_fts, rank) VALUES ('integrity-check', 1)");
      return "ok";
    });
    check("transaction_rollback", () => {
      const before = Number(rows("SELECT count(*) AS c FROM s_fts")[0].c);
      try {
        this.ctx.storage.transactionSync(() => {
          sql.exec("INSERT INTO s_fts (repo, number, title, body) VALUES (0, -1, 'rolled', 'back zulu')");
          throw new Error("abort");
        });
      } catch {}
      const after = Number(rows("SELECT count(*) AS c FROM s_fts")[0].c);
      const z = Number(rows("SELECT count(*) AS c FROM s_fts WHERE s_fts MATCH 'zulu'")[0].c);
      if (before !== after || z !== 0) throw new Error(JSON.stringify({ before, after, z }));
      return { before, after };
    });
    return { sqlite, options, colo: null, checks, dbBytes: sql.databaseSize };
  }

  /** A later request: the index persisted (in storage, not only in memory). */
  persisted(): { count: number; match: number } {
    const sql = this.ctx.storage.sql;
    const count = Number(sql.exec("SELECT count(*) AS c FROM s_fts").one().c);
    const match = Number(sql.exec("SELECT count(*) AS c FROM s_fts WHERE s_fts MATCH 'doc7'").one().c);
    return { count, match };
  }

  /** Where this object runs, from Cloudflare's trace endpoint. */
  async colo(): Promise<string> {
    const trace = await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text();
    return /^colo=(\w+)$/m.exec(trace)?.[1] ?? "unknown";
  }

  /** Evicts this object's isolate state; the next call loads from storage. */
  reset(): void {
    this.ctx.abort("c15 reset");
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.TOKEN}`) return new Response("no", { status: 401 });
    const url = new URL(req.url);
    const j = url.searchParams.get("j");
    const ns = j ? env.FTS.jurisdiction(j as DurableObjectJurisdiction) : env.FTS;
    const stub = ns.get(ns.idFromName(url.searchParams.get("n") ?? "c15"));
    const op = url.pathname.slice(1);
    let out: unknown;
    if (op === "run") out = await stub.run(Number(url.searchParams.get("docs") ?? "2000"));
    else if (op === "persisted") out = await stub.persisted();
    else if (op === "colo") out = await stub.colo();
    else if (op === "reset") out = await stub.reset().catch((e) => String(e));
    else return new Response("ops: run, persisted, reset", { status: 404 });
    return Response.json(out);
  },
};
