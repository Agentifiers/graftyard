// M0-WP10: `graft-m0-agent`, its own script (tests/m0/agent/wrangler.jsonc). One `AgentRepo` Durable Object holds the
// install's folds and runs `decide`; its container (tests/m0/agent/container) is the git cache stand-in, running real
// git (index-pack --strict, diff, rev-list, upload-pack) behind `gitd` with `enableInternet: false`, so git bytes
// reach it only through the object, as a git cache shard's do. Every request goes to that one object, which is the
// group leader of this prototype.
//
//   https://graft-m0-agent.<sub>.workers.dev/<owner>/<repo>.git/...   git smart HTTP (token as the Basic password)
//   .../api/v3/repos/...   .../api/graphql   .../api/graftyard/v1/repos/...
//   .../m0/setup | /m0/state | /m0/audit | /m0/fork-sync              harness, Bearer M0_TOKEN

//
// M0-WP13 adds, in front of the object (the edge, which no group member is):
//   .../api/graftyard/v1/repos/{o}/{r}/activity   the feed's edge checks (401, 403, 429 at FEED_POLLS); WP12 serves the rest
//   https://graft-m0-agent.<sub>.workers.dev/mcp/codemode   a prototype `graft-mcp` Code Mode route: model code runs in a
//                         Dynamic Worker whose only capability is `codemode.request()`, through the tool deny-list.

import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";
import { Host, type AuditRow, type Persist } from "./host.ts";
import type { Credential, Fold, RefUpdate } from "./model.ts";
import { BASE, MCP_CLIENT } from "./fixture.ts";
import { derive, tokenOf, verify } from "./token.ts";
import { FEED_POLLS, feedGate } from "./tasks.ts";
import { codeModeDenied } from "./mcp.ts";

interface Env { REPO: DurableObjectNamespace<AgentRepo>; M0_TOKEN: string; LOADER: WorkerLoader; SELF: Fetcher; LAYER?: string }

type Container = { running: boolean; images?: Record<string, string>; start(o: object): void; destroy(): Promise<void>;
                   getTcpPort(p: number): { fetch(url: string, init?: RequestInit): Promise<Response> }; setInactivityTimeout(ms: number): Promise<void> };

export class AgentRepo extends DurableObject<Env> {
  private host: Host | null = null;
  private get c(): Container { return this.ctx.container as unknown as Container; }

  private async up(): Promise<void> {
    if (!this.c.running) this.c.start({ image: this.c.images!.git, instance: "standard-2", enableInternet: false });
    await this.c.setInactivityTimeout(60 * 60_000);
    const port = this.c.getTcpPort(8080);
    for (let i = 0; i < 300; i++) {
      try { if ((await port.fetch("http://gitd/health")).ok) return; } catch { /* starting */ }
      await new Promise((r) => setTimeout(r, 200));
    }
    throw new Error("gitd did not answer within 60 s");
  }

  /** The folds live in this object's SQLite, as a member's do: refs one row each, the rest of each fold as JSON. */
  private persist(): Persist {
    const sql = this.ctx.storage.sql;
    sql.exec("CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT)");
    sql.exec("CREATE TABLE IF NOT EXISTS fold (repo TEXT PRIMARY KEY, v TEXT)");
    sql.exec("CREATE TABLE IF NOT EXISTS refs (repo TEXT, name TEXT, oid TEXT, PRIMARY KEY (repo, name))");
    sql.exec("CREATE TABLE IF NOT EXISTS audit (probe TEXT PRIMARY KEY, v TEXT)");
    sql.exec("CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT)");
    return {
      reset: (epoch) => { for (const t of ["meta", "fold", "refs", "audit", "kv"]) sql.exec(`DELETE FROM ${t}`); sql.exec("INSERT INTO meta VALUES ('epoch', ?)", String(epoch)); },
      saveKv: (k, v) => { sql.exec("INSERT OR REPLACE INTO kv VALUES (?, ?)", k, v); },
      saveFold: (repo, f) => { sql.exec("INSERT OR REPLACE INTO fold VALUES (?, ?)", repo, JSON.stringify({ ...f, refs: {} })); },
      saveRefs: (repo, us: RefUpdate[]) => { for (const u of us) {
        if (u.new == null) sql.exec("DELETE FROM refs WHERE repo = ? AND name = ?", repo, u.name);
        else sql.exec("INSERT OR REPLACE INTO refs VALUES (?, ?, ?)", repo, u.name, u.new); } },
      saveAudit: (row: AuditRow) => { sql.exec("INSERT OR REPLACE INTO audit VALUES (?, ?)", row.probe, JSON.stringify(row)); },
      load: () => {
        const e = sql.exec<{ v: string }>("SELECT v FROM meta WHERE k = 'epoch'").toArray()[0];
        if (!e) return null;
        sql.exec("INSERT INTO meta VALUES ('boots', '1') ON CONFLICT(k) DO UPDATE SET v = CAST(v AS INTEGER) + 1");
        const folds: Record<string, Fold> = {};
        for (const r of sql.exec<{ repo: string; v: string }>("SELECT repo, v FROM fold")) folds[r.repo] = JSON.parse(r.v);
        for (const r of sql.exec<{ repo: string; name: string; oid: string }>("SELECT repo, name, oid FROM refs")) folds[r.repo].refs[r.name] = r.oid;
        const kv: Record<string, string> = {};
        for (const r of sql.exec<{ k: string; v: string }>("SELECT k, v FROM kv")) kv[r.k] = r.v;
        return { epoch: Number(e.v), folds, kv };
      },
      audit: (probe) => { const r = sql.exec<{ v: string }>("SELECT v FROM audit WHERE probe = ?", probe).toArray()[0]; return r ? JSON.parse(r.v) : null; },
    };
  }

  private hostFor(url: URL): Host {
    if (!this.host) {
      const port = () => this.c.getTcpPort(8080);
      this.host = new Host(this.env.M0_TOKEN, { fetch: (path, init) => port().fetch(`http://gitd${path}`, init) }, url.host, this.persist(),
        { send: (u, init) => fetch(u, init), later: (p) => this.ctx.waitUntil(p) });
      this.host.layer = this.env.LAYER !== "off";
      if (!this.host.layer) this.host.authz.agentApps = [];
    }
    return this.host;
  }

  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    await this.up();
    const host = this.hostFor(url);
    if (url.pathname.startsWith("/m0/")) {
      if (req.headers.get("authorization") !== `Bearer ${this.env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
      if (url.pathname === "/m0/setup") return Response.json(await host.setup());
      if (url.pathname === "/m0/fork-sync") return Response.json(await host.forkSync());
      if (url.pathname === "/m0/state") return Response.json({ ...host.state(),
        boots: Number(this.ctx.storage.sql.exec<{ v: string }>("SELECT v FROM meta WHERE k = 'boots'").toArray()[0]?.v ?? 0) });
      if (url.pathname === "/m0/audit") return Response.json(host.auditRows(await req.json<string[]>()));
      if (url.pathname === "/m0/check") return Response.json(await host.check());
      if (url.pathname === "/m0/host-refs") return Response.json(host.hostRefs(url.searchParams.get("repo") ?? BASE));
      if (url.pathname === "/m0/token-write") return Response.json({ write: host.authz.tokens?.[url.searchParams.get("jti") ?? ""]?.write ?? null });
      if (url.pathname === "/m0/deliveries") return Response.json(host.deliveryLog());
      if (url.pathname === "/m0/prompt") return Response.json(host.prompt(url.searchParams.get("repo") ?? BASE, url.searchParams.get("session") ?? ""));
      if (url.pathname === "/m0/feed-at-leader") return Response.json({ feedAtLeader: host.feedAtLeader });
      if (url.pathname === "/m0/tasks") return Response.json(host.folds[url.searchParams.get("repo") ?? BASE]?.tasks ?? {});
      if (url.pathname === "/m0/pr") return Response.json(host.folds[url.searchParams.get("repo") ?? BASE]?.prs[url.searchParams.get("n") ?? ""] ?? null);
      if (url.pathname === "/m0/fold-set") { // harness: set a fold field the API cannot (rulesets, extra approvals)
        const b = await req.json<{ repo: string; path: string[]; value: unknown }>();
        let o: any = host.folds[b.repo]; for (const k of b.path.slice(0, -1)) o = o[k]; o[b.path.at(-1)!] = b.value;
        return Response.json({ ok: true });
      }
      if (url.pathname === "/m0/stop") { await this.c.destroy(); this.host = null; return Response.json({ stopped: true }); }
      return new Response("not found", { status: 404 });
    }
    if (!host.epoch) return new Response("run /m0/setup first\n", { status: 503 });
    return host.handle(req);
  }
}

// ---- The edge (M0-WP13) ----
/** Polls in flight per token and per client IP, in this isolate. A real edge counts across isolates; the L0-15 driver
 *  sends its over-limit polls through one connection so they share an isolate [prototype limit]. */
const inflight = { token: new Map<string, number>(), ip: new Map<string, number>() };
const FEED = /^\/api\/graftyard\/v1\/repos\/([^/]+)\/([^/]+)\/activity$/;

/** The feed's edge checks, before any member: 401 without a token, 403 without `contents: read`, 429 past FEED_POLLS.
 *  A poll that passes would go to a learner or follower (WP12); this prototype holds it for `wait` and answers empty. */
async function feedEdge(req: Request, env: Env, m: RegExpExecArray): Promise<Response> {
  const tok = tokenOf(req);
  const cred = tok ? await verify(env.M0_TOKEN, tok) : null;
  const repo = `${m[1]}/${m[2]}`;
  const contents = cred ? canReadContents(cred, repo) : false;
  const g = feedGate(cred && cred.t !== "AppJwt" ? cred : null, contents);
  const body = (status: number, message: string, h: Record<string, string> = {}) =>
    Response.json({ message, documentation_url: "https://docs.github.com/enterprise-server@3.22/rest" }, { status, headers: { "x-graftyard-answered-by": "edge", ...h } });
  if (g) return body(g.status, g.message, g.status === 401 ? { "www-authenticate": 'Bearer realm="graftyard"' } : {});
  const ip = req.headers.get("cf-connecting-ip") ?? "";
  const tk = tok!;
  const t = inflight.token.get(tk) ?? 0, i = inflight.ip.get(ip) ?? 0;
  if (t >= FEED_POLLS.token || i >= FEED_POLLS.ip) return body(429, "You have exceeded a secondary rate limit: at most 2 feed polls at once per token and 50 per IP", { "retry-after": "10" });
  inflight.token.set(tk, t + 1); inflight.ip.set(ip, i + 1);
  try {
    const wait = Math.min(25, Math.max(0, Number(new URL(req.url).searchParams.get("wait") ?? 0) || 0));
    await new Promise((r) => setTimeout(r, wait * 1000));
    return Response.json({ cursor: null, events: [] }, { headers: { "x-graftyard-answered-by": "edge-stand-in-for-learner" } });
  } finally {
    inflight.token.set(tk, (inflight.token.get(tk) ?? 1) - 1); inflight.ip.set(ip, (inflight.ip.get(ip) ?? 1) - 1);
  }
}
/** `contents: read` from the token alone, as the edge can tell it: a fine-grained PAT's or narrowed installation token's
 *  permission set, else the App's grant (fixture). A classic PAT or user-to-server token defers to the role (WP12). */
function canReadContents(c: Credential, repo: string): boolean {
  if (c.t === "Pat") return !c.perms || c.perms.contents != null;
  if (c.t === "Installation") return (!c.repos || c.repos.includes(repo)) && (c.perms ? c.perms.contents != null : true);
  if (c.t === "UserToServer") return !c.repos || c.repos.includes(repo);
  return false;
}

/** `codemode.request()`: the only capability the Dynamic Worker gets. It forwards to the edge with `upstream`, the
 *  user's scoped token, after the same tool deny-list as `call`. `ctx.props` carries the token, so the model code
 *  never sees it. */
export class CodeModeCap extends WorkerEntrypoint<Env, { upstream: string; write: string; probe: string }> {
  async request(method: string, path: string, body?: unknown): Promise<{ status: number; body: unknown }> {
    const deny = codeModeDenied(String(method).toUpperCase(), String(path), body, this.ctx.props.write);
    if (deny) return { status: 403, body: { message: deny, refused_by: "graft-mcp deny-list" } };
    const r = await this.env.REPO.get(this.env.REPO.idFromName("install"), { locationHint: "enam" }).fetch(new Request(`https://edge${path}`, {
      method, body: body === undefined ? undefined : JSON.stringify(body),
      headers: { authorization: `Bearer ${this.ctx.props.upstream}`, "content-type": "application/json", "x-m0-probe": this.ctx.props.probe } }));
    const text = await r.text();
    let j: unknown = text; try { j = JSON.parse(text); } catch { /* text */ }
    return { status: r.status, body: j };
  }
}

/** `/mcp/codemode`: `execute()` of model-written code. Served only while every repository's org has `code_mode` on,
 *  read through `GET /orgs/{org}/agents` with the Graftyard MCP client credentials. */
async function codeMode(req: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
  const tok = tokenOf(req);
  const cred = tok ? await verify(env.M0_TOKEN, tok) : null;
  if (!cred || cred.t !== "UserToServer" || !cred.jti) return Response.json({ error: "invalid_token" }, { status: 401, headers: { "www-authenticate": 'Bearer resource_metadata="/.well-known/oauth-protected-resource"' } });
  const b = await req.json<{ code: string; write?: string }>().catch(() => null);
  if (!b?.code) return Response.json({ error: "missing code" }, { status: 400 });
  const stub = env.REPO.get(env.REPO.idFromName("install"), { locationHint: "enam" });
  const basic = btoa(`${MCP_CLIENT.id}:${await derive(env.M0_TOKEN, MCP_CLIENT.secretLabel)}`);
  const org = await (await stub.fetch(new Request("https://edge/api/graftyard/v1/orgs/acme/agents", { headers: { authorization: `Basic ${basic}` } }))).json<{ code_mode: boolean }>();
  if (!org.code_mode) return Response.json({ error: "Code Mode is off for this organization" }, { status: 403 });
  const write = (await (await stub.fetch(new Request(`https://edge/m0/token-write?jti=${cred.jti}`, { headers: { authorization: `Bearer ${env.M0_TOKEN}` } }))).json<{ write: string | null }>()).write ?? "Read";
  const worker = env.LOADER.load({
    compatibilityDate: "2026-10-01", mainModule: "run.js",
    // The model's code becomes the body of an async function in the module, as Cloudflare's Code Mode does
    // (Workers forbid `eval` and `new Function`). Its only capability is `codemode`.
    modules: { "run.js": `export default { async fetch(req, env) {
      const codemode = env.codemode;
      try { const result = await (async () => {\n${b.code}\n})(); return Response.json({ ok: true, result }); }
      catch (e) { return Response.json({ ok: false, error: String(e) }); }
    } }` },
    env: { codemode: (ctx.exports as unknown as { CodeModeCap(o: { props: object }): Fetcher }).CodeModeCap({ props: { upstream: tok!, write, probe: req.headers.get("x-m0-probe") ?? "" } }) },
    globalOutbound: null,
  });
  return worker.getEntrypoint().fetch(new Request("https://codemode/run"));
}

export default {
  async fetch(req: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const url = new URL(req.url);
    const f = FEED.exec(url.pathname);
    if (env.LAYER !== "off" && f && req.method === "GET") return feedEdge(req, env, f);
    if (env.LAYER !== "off" && url.pathname === "/mcp/codemode" && req.method === "POST") return codeMode(req, env, ctx);
    return env.REPO.get(env.REPO.idFromName("install"), { locationHint: "enam" }).fetch(req);
  },
} satisfies ExportedHandler<Env>;
