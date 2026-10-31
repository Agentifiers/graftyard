// M0-WP10 bed: deploy or delete `graft-m0-agent` (src/agent/worker.ts, agent/wrangler.jsonc), or run the same `Host`
// locally with `gitd` as a child process for development.
//
//   node run/agent.ts deploy        wrangler deploy -c agent/wrangler.jsonc, then the M0_TOKEN secret
//   node run/agent.ts setup [--url U]
//   node run/agent.ts delete
//   node run/agent.ts serve [--port 8787]       local Host + gitd, for manual runs
//   node run/agent.ts deploy-hooks | delete-hooks   M0-WP13's graft-wp13-hooks (agent/hooks/wrangler.jsonc)

import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { createServer, type Server } from "node:http";
import { appendFileSync, mkdirSync, rmSync } from "node:fs";
import { parseArgs } from "node:util";
import { Host } from "../src/agent/host.ts";
import { ROOT, TOKEN, harness, baseUrl } from "./agent-lib.ts";

const wrangler = (args: string[], input?: string) =>
  execFileSync("npx", ["wrangler", ...args], { cwd: ROOT, input, encoding: "utf8", stdio: [input ? "pipe" : "ignore", "pipe", "inherit"] });

export interface Local { url: string; stop(): void }

/** Starts gitd (built from agent/container/gitd) on a free port and a node HTTP server around `Host`. */
export async function startLocal(port = 0): Promise<Local> {
  const gitdDir = `${ROOT}.gen/wp10-gitd`;
  rmSync(gitdDir, { recursive: true, force: true });
  mkdirSync(gitdDir, { recursive: true });
  execFileSync("go", ["build", "-o", `${gitdDir}/gitd`, "."], { cwd: `${ROOT}agent/container/gitd`, stdio: "inherit" });
  const gport = 18000 + Math.floor(Math.random() * 1000);
  const gitd: ChildProcess = spawn(`${gitdDir}/gitd`, ["-root", `${gitdDir}/data`], { env: { ...process.env, GITD_ADDR: `127.0.0.1:${gport}` }, stdio: "ignore" });
  for (let i = 0; i < 100; i++) {
    try { if ((await fetch(`http://127.0.0.1:${gport}/health`)).ok) break; } catch { await new Promise((r) => setTimeout(r, 100)); }
  }
  let host: Host | null = null;
  const server: Server = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const c of req) chunks.push(c as Buffer);
    const url = `http://${req.headers.host}${req.url}`;
    const headers = new Headers();
    for (const [k, v] of Object.entries(req.headers)) if (typeof v === "string") headers.set(k, v);
    const r = new Request(url, { method: req.method, headers, body: req.method === "GET" || req.method === "HEAD" ? undefined : Buffer.concat(chunks) });
    let out: Response;
    try {
      const u = new URL(url);
      if (!host) host = new Host(TOKEN, { fetch: (p, init) => fetch(`http://127.0.0.1:${gport}${p}`, init) }, u.host, null,
        { send: (to, init) => fetch(to, init), later: (p) => { p.catch(() => undefined); } });
      if (u.pathname.startsWith("/m0/")) {
        if (headers.get("authorization") !== `Bearer ${TOKEN}`) out = new Response("unauthorized", { status: 401 });
        else if (u.pathname === "/m0/setup") out = Response.json(await host.setup());
        else if (u.pathname === "/m0/fork-sync") out = Response.json(await host.forkSync());
        else if (u.pathname === "/m0/state") out = Response.json(host.state());
        else if (u.pathname === "/m0/audit") out = Response.json(host.auditRows(await r.json() as string[]));
        else if (u.pathname === "/m0/check") out = Response.json(await host.check());
        else if (u.pathname === "/m0/host-refs") out = Response.json(host.hostRefs(u.searchParams.get("repo") ?? ""));
        else if (u.pathname === "/m0/deliveries") out = Response.json(host.deliveryLog());
        else if (u.pathname === "/m0/prompt") out = Response.json(host.prompt(u.searchParams.get("repo") ?? "", u.searchParams.get("session") ?? ""));
        else if (u.pathname === "/m0/feed-at-leader") out = Response.json({ feedAtLeader: host.feedAtLeader });
        else if (u.pathname === "/m0/tasks") out = Response.json(host.folds[u.searchParams.get("repo") ?? ""]?.tasks ?? {});
        else if (u.pathname === "/m0/pr") out = Response.json(host.folds[u.searchParams.get("repo") ?? ""]?.prs[u.searchParams.get("n") ?? ""] ?? null);
        else if (u.pathname === "/m0/token-write") out = Response.json({ write: host.authz.tokens?.[u.searchParams.get("jti") ?? ""]?.write ?? null });
        else if (u.pathname === "/m0/fold-set") {
          const b = await r.json() as { repo: string; path: string[]; value: unknown };
          let o: any = host.folds[b.repo]; for (const k of b.path.slice(0, -1)) o = o[k]; o[b.path.at(-1)!] = b.value;
          out = Response.json({ ok: true });
        }
        else out = new Response("not found", { status: 404 });
      } else out = await host.handle(r);
    } catch (e) {
      appendFileSync(`${ROOT}.gen/wp10-local-errors.log`, `${new Date().toISOString()} ${req.method} ${req.url}\n${(e as Error).stack ?? e}\n`);
      out = new Response(String((e as Error).stack ?? e), { status: 500 });
    }
    res.writeHead(out.status, Object.fromEntries(out.headers));
    res.end(Buffer.from(await out.arrayBuffer()));
  });
  await new Promise<void>((r) => server.listen(port, "127.0.0.1", r));
  const addr = server.address() as { port: number };
  return { url: `http://127.0.0.1:${addr.port}`, stop: () => { server.close(); gitd.kill(); } };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: { url: { type: "string" }, port: { type: "string", default: "8787" } } });
  switch (positionals[0]) {
    case "deploy":
      process.stdout.write(wrangler(["deploy", "-c", "agent/wrangler.jsonc"]));
      process.stdout.write(wrangler(["secret", "put", "M0_TOKEN", "-c", "agent/wrangler.jsonc"], TOKEN));
      break;
    case "setup": console.log(JSON.stringify(await harness(baseUrl(opt), "/m0/setup", {}))); break;
    case "delete": process.stdout.write(wrangler(["delete", "--name", "graft-m0-agent", "--force"])); break;
    case "deploy-hooks":
      process.stdout.write(wrangler(["deploy", "-c", "agent/hooks/wrangler.jsonc"]));
      process.stdout.write(wrangler(["secret", "put", "M0_TOKEN", "-c", "agent/hooks/wrangler.jsonc"], TOKEN));
      break;
    case "delete-hooks": process.stdout.write(wrangler(["delete", "--name", "graft-wp13-hooks", "--force"])); break;
    case "serve": { const l = await startLocal(Number(opt.port)); console.log(`serving ${l.url}`); break; }
    default: console.error("usage: node run/agent.ts deploy|setup|delete|serve"); process.exit(2);
  }
}
