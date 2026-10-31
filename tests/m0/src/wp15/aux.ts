// M0-WP15: graft-wp15-aux. Two things the clients need on a public HTTPS origin:
//   GET /cimd/<name>.json      Client ID Metadata Documents, one per MCP client under test, `client_id` = its own URL
//   GET /cimd/big.json, /cimd/redirect.json, /cimd/wrong-id.json   negative CIMD cases
//   POST /hooks/<app>          webhook sink: verifies X-Hub-Signature-256 with the App's secret, stores the delivery
//   GET  /hooks/<app>          the stored deliveries (Bearer M0_TOKEN)
// Redirect URIs are loopback, as a native MCP client's are; the drivers catch the redirect without a browser.

import { DurableObject } from "cloudflare:workers";

interface Env { LOG: DurableObjectNamespace<HookLog>; M0_TOKEN: string }

export const CLIENTS = ["ts-v2", "py-v2", "go-v18", "ts-v1", "evil"] as const;

export class HookLog extends DurableObject<Env> {
  async fetch(req: Request): Promise<Response> {
    const sql = this.ctx.storage.sql;
    sql.exec("CREATE TABLE IF NOT EXISTS d (seq INTEGER PRIMARY KEY AUTOINCREMENT, app TEXT, at INTEGER, event TEXT, delivery TEXT, ok INTEGER, body TEXT)");
    const url = new URL(req.url);
    if (req.method === "POST") {
      const b = await req.json<{ app: string; event: string; delivery: string; ok: boolean; body: string }>();
      sql.exec("INSERT INTO d (app, at, event, delivery, ok, body) VALUES (?, ?, ?, ?, ?, ?)", b.app, Date.now(), b.event, b.delivery, b.ok ? 1 : 0, b.body);
      return new Response("ok");
    }
    if (req.method === "DELETE") { sql.exec("DELETE FROM d"); return new Response("ok"); }
    const rows = sql.exec("SELECT seq, app, at, event, delivery, ok, body FROM d WHERE app = ? ORDER BY seq", url.searchParams.get("app") ?? "").toArray();
    return Response.json(rows);
  }
}

async function hmacHex(key: string, data: string): Promise<string> {
  const k = await crypto.subtle.importKey("raw", new TextEncoder().encode(key), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return [...new Uint8Array(await crypto.subtle.sign("HMAC", k, new TextEncoder().encode(data)))].map((b) => b.toString(16).padStart(2, "0")).join("");
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    const m = /^\/cimd\/([a-z0-9-]+)\.json$/.exec(url.pathname);
    if (m && req.method === "GET") {
      const self = `${url.origin}${url.pathname}`;
      const doc = { client_id: self, client_name: `M0-WP15 ${m[1]}`, redirect_uris: [`http://127.0.0.1:47${CLIENTS.indexOf(m[1] as never) + 10}/callback`],
                    grant_types: ["authorization_code", "refresh_token"], response_types: ["code"], token_endpoint_auth_method: "none" };
      if (m[1] === "big") return Response.json({ ...doc, pad: "x".repeat(6_000) });
      if (m[1] === "redirect") return Response.redirect(`${url.origin}/cimd/ts-v2.json`, 302);
      if (m[1] === "wrong-id") return Response.json({ ...doc, client_id: `${url.origin}/cimd/ts-v2.json` });
      if (m[1] === "slow") { await new Promise((r) => setTimeout(r, 7_000)); return Response.json(doc); }
      if (!(CLIENTS as readonly string[]).includes(m[1])) return new Response("not found", { status: 404 });
      return Response.json(doc, { headers: { "cache-control": "max-age=300" } });
    }
    const hk = /^\/hooks\/([a-z0-9-]+)$/.exec(url.pathname);
    const log = env.LOG.get(env.LOG.idFromName("hooks"));
    if (hk && req.method === "POST") {
      const body = await req.text();
      const want = `sha256=${await hmacHex(await hmacHex(env.M0_TOKEN, `webhook:${hk[1]}`), body)}`;
      const ok = req.headers.get("x-hub-signature-256") === want;
      await log.fetch(new Request("https://log/", { method: "POST", body: JSON.stringify({ app: hk[1], event: req.headers.get("x-github-event") ?? "",
        delivery: req.headers.get("x-github-delivery") ?? "", ok, body }) }));
      return new Response(ok ? "ok" : "bad signature", { status: ok ? 200 : 401 });
    }
    if (hk && (req.method === "GET" || req.method === "DELETE")) {
      if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
      return log.fetch(new Request(`https://log/?app=${hk[1]}`, { method: req.method }));
    }
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<Env>;
