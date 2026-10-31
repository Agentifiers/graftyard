// M0-WP14 (L0-02): a per-script hit counter, so every stand-in target records, on the server side, each request that
// reached it. The harness classifies attempts from what the sandbox sees; these counters are the independent check
// that a request the router should refuse never arrived. Each wp14 target script exports `Hits` and binds it as HITS.

import { DurableObject } from "cloudflare:workers";

export class Hits extends DurableObject {
  async hit(kind: string): Promise<void> {
    const sql = this.ctx.storage.sql;
    sql.exec("CREATE TABLE IF NOT EXISTS hits (kind TEXT PRIMARY KEY, n INTEGER)");
    sql.exec("INSERT INTO hits VALUES (?, 1) ON CONFLICT(kind) DO UPDATE SET n = n + 1", kind);
  }
  async all(): Promise<Record<string, number>> {
    const sql = this.ctx.storage.sql;
    sql.exec("CREATE TABLE IF NOT EXISTS hits (kind TEXT PRIMARY KEY, n INTEGER)");
    const out: Record<string, number> = {};
    for (const r of sql.exec<{ kind: string; n: number }>("SELECT kind, n FROM hits")) out[r.kind] = r.n;
    return out;
  }
  async reset(): Promise<void> {
    this.ctx.storage.sql.exec("DROP TABLE IF EXISTS hits");
  }
}

export interface HitsEnv { HITS: DurableObjectNamespace<Hits>; M0_TOKEN: string }

/** Serves `GET /m0/hits` and `POST /m0/hits/reset` for the harness (Bearer M0_TOKEN); null for any other path. */
export async function hitsRoute(req: Request, env: HitsEnv): Promise<Response | null> {
  const p = new URL(req.url).pathname;
  if (p !== "/m0/hits" && p !== "/m0/hits/reset") return null;
  if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
  const stub = env.HITS.getByName("hits");
  if (p === "/m0/hits/reset" && req.method === "POST") { await stub.reset(); return new Response("reset"); }
  return Response.json(await stub.all());
}

export const record = (env: HitsEnv, kind: string): Promise<void> => env.HITS.getByName("hits").hit(kind);
