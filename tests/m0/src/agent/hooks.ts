// M0-WP13: `graft-wp13-hooks`, standing in for every App's webhook server. `POST /<app-slug>` records the delivery
// (App, event, delivery id, arrival time, and whether `X-Hub-Signature-256` verifies under that App's secret).
// `GET /log?since=` (Bearer M0_TOKEN) returns the rows; `POST /reset` clears them.

import { DurableObject } from "cloudflare:workers";
import { derive, hmac } from "./token.ts";
import { APP } from "./fixture.ts";

interface Env { LOG: DurableObjectNamespace<HookLog>; M0_TOKEN: string }

export class HookLog extends DurableObject<Env> {
  async fetch(req: Request): Promise<Response> {
    const sql = this.ctx.storage.sql;
    sql.exec("CREATE TABLE IF NOT EXISTS d (seq INTEGER PRIMARY KEY AUTOINCREMENT, app TEXT, event TEXT, id TEXT, at INTEGER, sig INTEGER, body TEXT)");
    const url = new URL(req.url);
    if (url.pathname === "/reset") { sql.exec("DELETE FROM d"); return Response.json({ ok: true }); }
    if (url.pathname === "/log") return Response.json(sql.exec("SELECT app, event, id, at, sig, body FROM d WHERE at >= ? ORDER BY seq", Number(url.searchParams.get("since") ?? 0)).toArray());
    const b = await req.json<{ app: string; event: string; id: string; at: number; sig: number; body: string }>();
    sql.exec("INSERT INTO d (app, event, id, at, sig, body) VALUES (?, ?, ?, ?, ?, ?)", b.app, b.event, b.id, b.at, b.sig, b.body);
    return Response.json({ ok: true });
  }
}

const SLUG_APP: Record<string, number> = { "graftyard-agent": APP.agent, claude: APP.claude, cursor: APP.cursor, "graftyard-insights": APP.insights };

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    const log = env.LOG.get(env.LOG.idFromName("log"));
    if (url.pathname === "/log" || url.pathname === "/reset") {
      if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
      return log.fetch(req);
    }
    const slug = url.pathname.slice(1);
    const app = SLUG_APP[slug];
    if (req.method !== "POST" || app == null) return new Response("not found", { status: 404 });
    const at = Date.now();
    const body = await req.text();
    const want = [...await hmac(await derive(env.M0_TOKEN, `webhook:${app}`), body)].map((x) => x.toString(16).padStart(2, "0")).join("");
    const sig = req.headers.get("x-hub-signature-256") === `sha256=${want}` ? 1 : 0;
    await log.fetch(new Request("https://log/add", { method: "POST", body: JSON.stringify({ app: slug, event: req.headers.get("x-github-event") ?? "",
      id: req.headers.get("x-github-delivery") ?? "", at, sig, body: body.slice(0, 2000) }) }));
    return new Response(null, { status: 204 });
  },
} satisfies ExportedHandler<Env>;
