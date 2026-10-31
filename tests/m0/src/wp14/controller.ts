// M0-WP14 (L0-02): `graft-wp14-controller`, the controller prototype. One Durable Object class, `Sandbox`, starts a real
// Sandbox (a `durable_object`-policy container, `standard-2`, `enableInternet: false`) and runs the containment harness
// (wp14/harness/attempts.py) in it. Every HTTP and HTTPS request from the container goes to `OutboundRouter`
// (src/wp14/router.ts), a `WorkerEntrypoint` of this script registered with `interceptOutboundHttps("*")` and
// `interceptAllOutboundHttp()`. The router reads the two installation tokens and the model key from this script's env,
// so they never enter the sandbox (research/agents.md 6.7; design/AGENT-LAYER.md, A3 step 5).
//
// The harness runs in the background inside the container and writes one JSON file per escape class under /work/out,
// so a dropped client, an evicted object, or an orb reboot loses nothing: `run` skips classes already written.
// `scan` is the object's own check that no credential reached the sandbox: it streams the container's environment,
// every process's environ and cmdline, and every regular file under 2 MB out through exec, and searches the bytes here,
// outside the sandbox, for the token and key values. The values never go into the container, not even as a grep argument.
//
//   POST /run     body: harness config (JSON)  start or resume the harness; returns at once
//   GET  /status                               which classes are done, the harness's stderr tail
//   GET  /result                               every class file and the positive controls, as one JSON object
//   POST /scan                                 the token scan; returns counts of each secret found
//   POST /stop                                 destroy the container
// Every route needs `Authorization: Bearer M0_TOKEN`.

import { DurableObject } from "cloudflare:workers";
import { OutboundRouter, type RouterProps } from "./router.ts";
// The harness source, inlined at build time by run/wp14.ts (`wrangler deploy` bundles this text import).
import HARNESS from "../../wp14/harness/attempts.py";

export { OutboundRouter };

interface Env {
  SANDBOX: DurableObjectNamespace<Sandbox>;
  M0_TOKEN: string; AGENT_TOKEN: string; WORKSPACE_TOKEN: string; MODEL_KEY: string;
  HOST_NAME: string; MODEL_HOST: string; REGISTRY_HOST: string;
}

const CA = "/etc/cloudflare/certs/cloudflare-containers-ca.crt";

function props(env: Env): RouterProps {
  return {
    hostName: env.HOST_NAME, modelHost: env.MODEL_HOST, registryHost: env.REGISTRY_HOST,
    base: "acme/app", pushTarget: "acme-agents/app", prefix: "refs/heads/graftyard/wp14-task/",
    modelPath: "/v1/chat/completions", goKnownModules: ["github.com/graftyard/known"],
  };
}

const dec = new TextDecoder();
// The harness's process is a child of pid 1 (`sleep infinity`), which never reaps it, so a finished or crashed
// harness stays a zombie that `kill -0` still answers. Alive means present and not in state Z. The pid must be checked
// non-empty first: with no pid file, `/proc//stat` is `/proc/stat`, which exists.
const ALIVE = "p=$(cat /work/pid 2>/dev/null); [ -n \"$p\" ] && s=$(cut -d' ' -f3 /proc/$p/stat 2>/dev/null) && [ -n \"$s\" ] && [ \"$s\" != Z ] && echo alive";

export class Sandbox extends DurableObject<Env> {
  private get c(): Container { return this.ctx.container as unknown as Container; }

  private async up(): Promise<{ startMs: number; fresh: boolean }> {
    const t0 = Date.now();
    // `ctx.exports` is typed only by `wrangler types`, which this harness does not run, so name the one export here.
    const exports = this.ctx.exports as unknown as { OutboundRouter(o: { props: RouterProps }): Fetcher };
    const router = exports.OutboundRouter({ props: props(this.env) });
    const fresh = !this.c.running;
    if (fresh) this.c.start({ image: this.c.images.sandbox, instance: "standard-2", enableInternet: false });
    // Intercepts last until the container stops, so they are registered on every call; registering again is a no-op
    // for the entry count and replaces the handler.
    await this.c.interceptOutboundHttps("*", router);
    await this.c.interceptAllOutboundHttp(router);
    await this.c.setInactivityTimeout(60 * 60_000);
    for (let i = 0; i < 240; i++) {
      try { if ((await (await this.c.exec(["true"])).output()).exitCode === 0) return { startMs: Date.now() - t0, fresh }; }
      catch { await new Promise((r) => setTimeout(r, 250)); }
    }
    throw new Error("the container did not accept exec within 60 s");
  }

  private async sh(cmd: string, stdin?: string): Promise<{ code: number; out: string; err: string }> {
    const p = await this.c.exec(["sh", "-c", cmd], stdin === undefined ? {} : { stdin: new Blob([stdin]).stream() });
    const o = await p.output();
    return { code: o.exitCode, out: dec.decode(o.stdout), err: dec.decode(o.stderr) };
  }

  async run(config: Record<string, unknown>): Promise<object> {
    const up = await this.up();
    await this.sh("mkdir -p /work/out && cat > /work/attempts.py", HARNESS);
    await this.sh("cat > /work/config.json", JSON.stringify({ ...config, ca: CA }));
    const running = (await this.sh(ALIVE)).out.includes("alive");
    // `fresh: true` starts every class over, for a final run whose server-side counters cover that run alone.
    if (config.fresh === true && !running) await this.sh("rm -rf /work/out /work/base /tmp/src && mkdir -p /work/out && : > /work/harness.log");
    if (!running) await this.sh("cd /work && (nohup python3 attempts.py config.json >>/work/harness.log 2>&1 & echo $! > /work/pid)");
    return { started: !running, alreadyRunning: running, ...up };
  }

  async status(): Promise<object> {
    if (!this.c.running) return { running: false };
    const r = await this.sh("ls /work/out 2>/dev/null; echo ---; tail -c 3000 /work/harness.log 2>/dev/null; echo ---; " + ALIVE);
    const [ls, log, alive] = r.out.split("---\n");
    return { running: true, done: ls.split("\n").filter(Boolean), log, alive: (alive ?? "").includes("alive") };
  }

  async result(): Promise<string> {
    const r = await this.sh("cd /work/out && for f in *.json; do printf '%s\\t' \"$f\"; tr -d '\\n' < \"$f\"; echo; done");
    const all: Record<string, unknown> = {};
    for (const line of r.out.split("\n").filter(Boolean)) {
      const [name, body] = line.split("\t");
      try { all[name.replace(/\.json$/, "")] = JSON.parse(body); } catch { all[name] = { unparsed: body.slice(0, 200) }; }
    }
    return JSON.stringify(all);
  }

  /** Streams the sandbox's environment, /proc, and files out and searches them here for the real secret values. */
  async scan(): Promise<object> {
    await this.up();
    const secrets: Record<string, string> = { AGENT_TOKEN: this.env.AGENT_TOKEN, WORKSPACE_TOKEN: this.env.WORKSPACE_TOKEN, MODEL_KEY: this.env.MODEL_KEY };
    const forms = (v: string): string[] => [v, btoa(`x-access-token:${v}`), btoa(v)];
    const counts: Record<string, number> = Object.fromEntries(Object.keys(secrets).map((k) => [k, 0]));
    let bytes = 0, files = 0;
    const scanText = (t: string) => { bytes += t.length; for (const [k, v] of Object.entries(secrets)) for (const f of forms(v)) if (t.includes(f)) counts[k]++; };
    // Environment, every process's environ and cmdline (the harness's processes included), and git's config.
    scanText((await this.sh("env; for p in /proc/[0-9]*; do cat $p/environ $p/cmdline 2>/dev/null; echo; done; git config --global -l 2>/dev/null")).out);
    // Every regular file under 2 MB outside /proc, /sys, and /dev, in batches, so no single exec output is huge.
    const list = (await this.sh("find / -xdev \\( -path /proc -o -path /sys -o -path /dev \\) -prune -o -type f -size -2048k -print 2>/dev/null")).out.split("\n").filter(Boolean);
    for (let i = 0; i < list.length; i += 400) {
      const batch = list.slice(i, i + 400);
      const p = await this.c.exec(["sh", "-c", "xargs -0 cat 2>/dev/null"], { stdin: new Blob([batch.join("\0")]).stream() });
      scanText(dec.decode((await p.output()).stdout));
      files += batch.length;
    }
    return { counts, files, bytes };
  }

  async stop(): Promise<void> { if (this.c.running) await this.c.destroy().catch(() => {}); }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url);
    const s = env.SANDBOX.getByName("wp14");
    try {
      if (url.pathname === "/run" && req.method === "POST") return Response.json(await s.run(await req.json()));
      if (url.pathname === "/status") return Response.json(await s.status());
      if (url.pathname === "/result") return new Response(await s.result(), { headers: { "content-type": "application/json" } });
      if (url.pathname === "/scan" && req.method === "POST") return Response.json(await s.scan());
      if (url.pathname === "/stop" && req.method === "POST") { await s.stop(); return new Response("stopped\n"); }
    } catch (e) {
      return Response.json({ error: String(e) }, { status: 500 });
    }
    return new Response("POST /run, GET /status, GET /result, POST /scan, POST /stop\n", { status: 404 });
  },
};

type Container = {
  running: boolean; images: Record<string, string>;
  start(o: object): void; destroy(): Promise<void>;
  setInactivityTimeout(ms: number): Promise<void>;
  interceptOutboundHttps(addr: string, f: Fetcher): Promise<void>;
  interceptAllOutboundHttp(f: Fetcher): Promise<void>;
  exec(cmd: string[], o?: object): Promise<{ output(): Promise<{ exitCode: number; stdout: ArrayBuffer; stderr: ArrayBuffer }> }>;
};
