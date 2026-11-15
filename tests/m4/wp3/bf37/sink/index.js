// BF-37's hook receiver: every delivery to /h/<name> is kept whole (headers and body) in one object, so the
// go-playground/webhooks replay (`../main.go`) parses exactly what the edge sent. /dump answers them in arrival order.
export class Log {
  constructor(state) {
    this.sql = state.storage.sql;
    this.sql.exec("CREATE TABLE IF NOT EXISTS d (at INTEGER, hook TEXT, event TEXT, guid TEXT, sig TEXT, body TEXT)");
  }
  async fetch(req) {
    const u = new URL(req.url);
    if (u.pathname === "/add") {
      const r = await req.json();
      this.sql.exec("INSERT INTO d VALUES (?,?,?,?,?,?)", r.at, r.hook, r.event, r.guid, r.sig, r.body);
      return new Response("ok");
    }
    if (u.pathname === "/dump") return Response.json([...this.sql.exec("SELECT * FROM d ORDER BY at")]);
    if (u.pathname === "/clear") { this.sql.exec("DELETE FROM d"); return new Response("ok"); }
    return new Response("?", { status: 404 });
  }
}
export default {
  async fetch(req, env) {
    const u = new URL(req.url);
    const log = env.LOG.get(env.LOG.idFromName("log"));
    if (u.pathname === "/dump" || u.pathname === "/clear") return log.fetch(req);
    if (!u.pathname.startsWith("/h/") || req.method !== "POST") return new Response("?", { status: 404 });
    const h = (k) => req.headers.get(k);
    const row = { at: Date.now(), hook: h("x-github-hook-id"), event: h("x-github-event"), guid: h("x-github-delivery"),
      sig: h("x-hub-signature-256"), body: await req.text() };
    await log.fetch("https://l/add", { method: "POST", body: JSON.stringify(row) });
    return new Response("ok");
  },
};
