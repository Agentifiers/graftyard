// M0-WP7 (B0-24): `graft-m0-ssh`, the SSH front prototype (design/DESIGN.md, Shape, "SSH front"; journey 4, step 3).
//
// `SshFront` is the SSH front container class under the `durable_object` scheduling policy. Three objects, `front-a`
// (enam), `front-b` (weur), and `front-c` (apac), each run one container (wp7/front/Dockerfile) holding OpenSSH, which
// stands in for `graft-sshd`, and one cloudflared replica of the same Tunnel. The container's outbound HTTP to the
// virtual hosts `keys.graft`, `tokens.graft`, and `group.graft` goes to `SshOutbound`, a `WorkerEntrypoint` of this
// script, which plays `ssh_outbound`: it runs in the Workers runtime and reaches the `Registry` objects through this
// script's own Durable Object binding, never through the public zone.
//
//   keys.graft/host-ed25519   the service host key (secret SSH_HOST_KEY), so every replica serves the same fingerprint
//   keys.graft/tunnel-token   the Tunnel token (secret TUNNEL_TOKEN)
//   tokens.graft/authorized   sshd's AuthorizedKeysCommand: the key line registered for a fingerprint, else 404
//   group.graft/commit        the pre-receive hook's write: resolve the key on `tokens`, then commit on `group`
//
// The front object is its container's supervisor: an alarm every few seconds restarts a container that is not running,
// as the design needs when "Cloudflare does not guarantee" to keep containers running. Each start and kill is logged.
//
// Harness routes (bearer M0_TOKEN): POST /front/<id>/start | /kill | /stop, GET /front/<id>/status, POST /keys,
// GET /group/log?since=<seq>, GET /group/count.

import { DurableObject, WorkerEntrypoint } from "cloudflare:workers";

interface Env {
  FRONT: DurableObjectNamespace<SshFront>;
  REGISTRY: DurableObjectNamespace<Registry>;
  M0_TOKEN: string; SSH_HOST_KEY: string; TUNNEL_TOKEN: string;
}

export const FRONTS: Record<string, DurableObjectLocationHint> = { "front-a": "enam", "front-b": "weur", "front-c": "apac" };
const SUPERVISE_MS = 2_000;

type Container = {
  running: boolean; images: Record<string, string>;
  start(o: object): void; destroy(): Promise<void>;
  setInactivityTimeout(ms: number): Promise<void>;
  interceptOutboundHttp(addr: string, f: Fetcher): Promise<void>;
  exec(cmd: string[], o?: object): Promise<{ output(): Promise<{ exitCode: number; stdout: ArrayBuffer; stderr: ArrayBuffer }> }>;
};

export class SshOutbound extends WorkerEntrypoint<Env, { front: string }> {
  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const reg = (name: string) => this.env.REGISTRY.get(this.env.REGISTRY.idFromName(name));
    if (url.hostname === "keys.graft") {
      if (url.pathname === "/host-ed25519") return new Response(this.env.SSH_HOST_KEY.trim() + "\n");
      if (url.pathname === "/tunnel-token") return new Response(this.env.TUNNEL_TOKEN.trim());
    }
    if (url.hostname === "tokens.graft" && url.pathname === "/authorized") {
      const line = await reg("tokens").lookup(url.searchParams.get("fp") ?? "");
      return line ? new Response(line + "\n") : new Response("", { status: 404 });
    }
    if (url.hostname === "group.graft" && url.pathname === "/commit" && req.method === "POST") {
      const fp = req.headers.get("x-graft-key") ?? "";
      if (!(await reg("tokens").lookup(fp))) return new Response("unknown key\n", { status: 403 });
      const seq = await reg("group").commit(this.ctx.props.front, fp, await req.text());
      return new Response(`seq=${seq}\n`);
    }
    return new Response("no such virtual host\n", { status: 404 });
  }
}

/** Stand-in for the `Tokens` and repository groups: one object named `tokens`, one named `group`. */
export class Registry extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS keys (fp TEXT PRIMARY KEY, line TEXT)");
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS log (seq INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER, front TEXT, fp TEXT, updates TEXT)");
  }
  async addKey(fp: string, line: string): Promise<void> {
    this.ctx.storage.sql.exec("INSERT OR REPLACE INTO keys VALUES (?, ?)", fp, line);
  }
  async lookup(fp: string): Promise<string | null> {
    return (this.ctx.storage.sql.exec("SELECT line FROM keys WHERE fp = ?", fp).toArray()[0]?.line as string) ?? null;
  }
  async commit(front: string, fp: string, updates: string): Promise<number> {
    return this.ctx.storage.sql.exec("INSERT INTO log (at, front, fp, updates) VALUES (?, ?, ?, ?) RETURNING seq",
      Date.now(), front, fp, updates).one().seq as number;
  }
  async log(since: number): Promise<object[]> {
    return this.ctx.storage.sql.exec("SELECT seq, at, front, updates FROM log WHERE seq > ? ORDER BY seq LIMIT 5000", since).toArray();
  }
  async count(): Promise<number> {
    return this.ctx.storage.sql.exec("SELECT count(*) AS n FROM log").one().n as number;
  }
}

export class SshFront extends DurableObject<Env> {
  private intercepted = false;
  private get c(): Container { return this.ctx.container as unknown as Container; }

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS events (at INTEGER, kind TEXT, detail TEXT)");
  }

  private note(kind: string, detail = ""): void {
    this.ctx.storage.sql.exec("INSERT INTO events VALUES (?, ?, ?)", Date.now(), kind, detail);
  }

  private async ensure(name: string): Promise<boolean> {
    const fresh = !this.c.running;
    if (fresh) {
      this.c.start({ image: this.c.images.front, instance: "basic", enableInternet: true, env: { GRAFT_FRONT: name } });
      this.intercepted = false;
      this.note("start");
    }
    if (!this.intercepted) {
      const exports = this.ctx.exports as unknown as { SshOutbound(o: { props: { front: string } }): Fetcher };
      const out = exports.SshOutbound({ props: { front: name } });
      for (const h of ["keys.graft", "tokens.graft", "group.graft"]) await this.c.interceptOutboundHttp(h, out);
      this.intercepted = true;
    }
    await this.c.setInactivityTimeout(5 * 3600_000); // the platform caps it at 6 hours
    return fresh;
  }

  async startFront(name: string): Promise<object> {
    await this.ctx.storage.put({ wanted: true, name });
    const fresh = await this.ensure(name);
    await this.ctx.storage.setAlarm(Date.now() + SUPERVISE_MS);
    return { fresh, running: this.c.running };
  }

  async alarm(): Promise<void> {
    const { wanted, name } = Object.fromEntries(await this.ctx.storage.get(["wanted", "name"])) as { wanted?: boolean; name?: string };
    if (!wanted || !name) return;
    try { if (await this.ensure(name)) this.note("restart", "supervisor"); } catch (e) { this.note("start-error", String(e)); }
    await this.ctx.storage.setAlarm(Date.now() + SUPERVISE_MS);
  }

  async kill(): Promise<object> {
    const t0 = Date.now();
    const was = this.c.running;
    if (was) await this.c.destroy().catch(() => {});
    this.note("kill", String(was));
    return { wasRunning: was, destroyMs: Date.now() - t0, at: t0 };
  }

  async stop(): Promise<void> {
    await this.ctx.storage.put("wanted", false);
    await this.ctx.storage.deleteAlarm();
    if (this.c.running) await this.c.destroy().catch(() => {});
    this.note("stop");
  }

  async shell(cmd: string): Promise<object> {
    if (!this.c.running) return { running: false };
    const o = await (await this.c.exec(["sh", "-c", cmd])).output();
    const d = new TextDecoder();
    return { code: o.exitCode, out: d.decode(o.stdout), err: d.decode(o.stderr) };
  }

  async status(): Promise<object> {
    const running = this.c.running;
    let ready: string | null = null, colo: string | null = null;
    if (running) {
      try {
        const o = await (await this.c.exec(["sh", "-c",
          "curl -s --max-time 2 127.0.0.1:2000/ready; echo; tr '\\0' '\\n' < /proc/1/environ | grep -E '^CLOUDFLARE_(LOCATION|REGION)='"])).output();
        const text = new TextDecoder().decode(o.stdout);
        ready = text.split("\n")[0];
        colo = text.split("\n").slice(1).join(" ").trim();
      } catch (e) { ready = `exec-error: ${e}`; }
    }
    const events = this.ctx.storage.sql.exec("SELECT at, kind, detail FROM events ORDER BY at DESC LIMIT 12").toArray();
    return { running, ready, colo, events };
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized\n", { status: 401 });
    const url = new URL(req.url);
    const reg = (name: string) => env.REGISTRY.get(env.REGISTRY.idFromName(name));
    try {
      const m = /^\/front\/(front-[abc])\/(start|kill|stop|status)$/.exec(url.pathname);
      if (m) {
        const [, name, op] = m;
        const f = env.FRONT.get(env.FRONT.idFromName(name), { locationHint: FRONTS[name] });
        if (op === "start") return Response.json(await f.startFront(name));
        if (op === "kill") return Response.json(await f.kill());
        if (op === "stop") { await f.stop(); return Response.json({ stopped: true }); }
        if (req.method === "POST" && op === "status") return Response.json(await f.shell(await req.text()));
        return Response.json(await f.status());
      }
      if (url.pathname === "/keys" && req.method === "POST") {
        const { fp, line } = await req.json() as { fp: string; line: string };
        await reg("tokens").addKey(fp, line);
        return Response.json({ ok: true });
      }
      if (url.pathname === "/group/log") return Response.json(await reg("group").log(Number(url.searchParams.get("since") ?? 0)));
      if (url.pathname === "/group/count") return Response.json({ count: await reg("group").count() });
    } catch (e) {
      return Response.json({ error: String(e) }, { status: 500 });
    }
    return new Response("not found\n", { status: 404 });
  },
};
