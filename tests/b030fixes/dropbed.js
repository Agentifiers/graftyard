// (i-3) local check: databaseSize of a member that holds its own metadata tables, drops them, then loads a scratch
// tier, against one that keeps them. Rows are 1.8 KB blobs, as the comment table's are.
export class M {
  constructor(state) { this.s = state.storage; this.sql = state.storage.sql; }
  size() { return this.sql.databaseSize; }
  fill(table, mb, base = 0) {
    this.sql.exec(`CREATE TABLE IF NOT EXISTS "${table}" (k0 INTEGER PRIMARY KEY, written INTEGER NOT NULL, v BLOB NOT NULL)`);
    const n = Math.floor((mb << 20) / 1800);
    const blob = new Uint8Array(1800).map((_, i) => (i * 31) & 255);
    for (let i = 0; i < n; i += 2000) {
      this.s.transactionSync(() => {
        for (let j = i; j < Math.min(n, i + 2000); j++) this.sql.exec(`INSERT INTO "${table}" VALUES (?, 1, ?)`, base + j, blob);
      });
    }
    return n;
  }
  async fetch(req) {
    const u = new URL(req.url); const mb = +u.searchParams.get("mb") || 64; const drop = u.searchParams.get("drop") === "1";
    const out = { mb, drop };
    this.fill("log", mb);                      // the member's log and core: kept either way
    this.fill("m_comment", 2 * mb);            // its own metadata tier
    out.own = this.size();
    if (drop) { this.sql.exec(`DROP TABLE "m_comment"`); this.sql.exec(`CREATE TABLE "m_comment" (k0 INTEGER PRIMARY KEY, written INTEGER NOT NULL, v BLOB NOT NULL)`); }
    out.afterDrop = this.size();
    this.fill("im_comment", 2 * mb, 1e9);      // the install's scratch tier, the same size
    out.peak = this.size();
    return Response.json(out);
  }
}
export default { async fetch(req, env) { const u = new URL(req.url); return env.M.get(env.M.idFromName(u.pathname + u.search)).fetch(req); } };
