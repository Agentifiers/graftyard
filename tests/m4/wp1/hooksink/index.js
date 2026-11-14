// BF-57/58 hook receiver: /h/<hook>?slow=1 waits 11 s, ?slow=<ms> that long; every arrival is recorded in one object.
export class Log {
  constructor(state) {
    this.sql = state.storage.sql;
    this.sql.exec("CREATE TABLE IF NOT EXISTS a (at INTEGER, hook TEXT, guid TEXT, event TEXT, ref TEXT, after TEXT, sig INTEGER)");
  }
  async fetch(req) {
    const u = new URL(req.url);
    if (u.pathname === "/add") {
      const r = await req.json();
      this.sql.exec("INSERT INTO a VALUES (?,?,?,?,?,?,?)", r.at, r.hook, r.guid, r.event, r.ref, r.after, r.sig);
      return new Response("ok");
    }
    if (u.pathname === "/dump") return Response.json([...this.sql.exec("SELECT * FROM a ORDER BY at")]);
    if (u.pathname === "/clear") { this.sql.exec("DELETE FROM a"); return new Response("ok"); }
    return new Response("?", { status: 404 });
  }
}
export default {
  async fetch(req, env, ctx) {
    const u = new URL(req.url);
    const log = env.LOG.get(env.LOG.idFromName("log"));
    if (u.pathname === "/dump" || u.pathname === "/clear") return log.fetch(req);
    const m = u.pathname.match(/^\/h\/(\d+)$/);
    if (!m) return new Response("?", { status: 404 });
    const at = Date.now();
    const text = await req.text();
    let ref = null, after = null;
    try { const b = JSON.parse(text); ref = b.repository?.name ?? null; after = b.description ?? b.comment?.body ?? b.after ?? null; } catch {}
    const row = { at, hook: req.headers.get("x-github-hook-id"), guid: req.headers.get("x-github-delivery"),
      event: req.headers.get("x-github-event"), ref, after, sig: req.headers.get("x-hub-signature-256") ? 1 : 0 };
    ctx.waitUntil(log.fetch("https://l/add", { method: "POST", body: JSON.stringify(row) }));
    const slow = u.searchParams.get("slow"); if (slow) await new Promise((r) => setTimeout(r, slow === "1" ? 11_000 : Number(slow)));
    return new Response("ok");
  },
};
