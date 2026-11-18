// M0-WP14 (L0-02): deploy, run, and tear down the containment bed. Every script is `graft-wp14-*`.
//
//   node run/wp14.ts deploy     deploy canary, model, registry, host, controller; mint and set the secrets once
//   node run/wp14.ts run [--fresh]  start or resume the harness in the Sandbox (returns at once); --fresh redoes every
//                               class and zeroes the targets' hit counters
//   node run/wp14.ts wait       poll until every class is done, then collect
//   node run/wp14.ts collect    write results/raw/wp14-l0-02.json: harness classes, the token scan, and server hit counts
//   node run/wp14.ts delete     destroy the Sandbox and delete every graft-wp14-* script
//
// Secrets live in .gen/wp14-secrets.json (gitignored): AGENT_TOKEN, WORKSPACE_TOKEN, MODEL_KEY, random per bed. The
// controller and host get the tokens, the model gets the key; the Sandbox never does.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { resolve4, resolve6 } from "node:dns/promises";

const ROOT = new URL("../", import.meta.url).pathname;
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const SUB = `${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const url = (n: string) => `https://graft-wp14-${n}.${SUB}`;
const SCRIPTS = ["canary", "model", "registry", "host", "controller"] as const;
const SECRETS = `${ROOT}.gen/wp14-secrets.json`;
const auth = { authorization: `Bearer ${TOKEN}` };

const wrangler = (args: string[], input?: string) =>
  execFileSync("npx", ["wrangler", ...args], { cwd: ROOT, input, encoding: "utf8", stdio: [input ? "pipe" : "ignore", "inherit", "inherit"] });

function secrets(): Record<string, string> {
  if (existsSync(SECRETS)) return JSON.parse(readFileSync(SECRETS, "utf8"));
  mkdirSync(`${ROOT}.gen`, { recursive: true });
  const s = { AGENT_TOKEN: `ghs_${randomBytes(18).toString("hex")}`, WORKSPACE_TOKEN: `ghs_${randomBytes(18).toString("hex")}`,
              MODEL_KEY: `sk-wp14-${randomBytes(18).toString("hex")}` };
  writeFileSync(SECRETS, JSON.stringify(s));
  return s;
}

function deploy(): void {
  const s = secrets();
  const want: Record<string, string[]> = {
    canary: ["M0_TOKEN"], model: ["M0_TOKEN", "MODEL_KEY"], registry: ["M0_TOKEN"],
    host: ["M0_TOKEN", "AGENT_TOKEN", "WORKSPACE_TOKEN"], controller: ["M0_TOKEN", "AGENT_TOKEN", "WORKSPACE_TOKEN", "MODEL_KEY"],
  };
  for (const n of SCRIPTS) {
    const cfg = `wp14/graft-wp14-${n}.jsonc`;
    wrangler(["deploy", "-c", cfg]);
    const all: Record<string, string> = { ...s, M0_TOKEN: TOKEN };
    const body = JSON.stringify(Object.fromEntries(want[n].map((k) => [k, all[k]])));
    wrangler(["secret", "bulk", "-c", cfg], body);
  }
}

async function call(path: string, init: RequestInit = {}, timeoutMs = 600_000): Promise<any> {
  for (let attempt = 0; ; attempt++) {
    const r = await fetch(url("controller") + path, { ...init, headers: { ...auth, ...(init.headers ?? {}) }, signal: AbortSignal.timeout(timeoutMs) }).catch((e) => e as Error);
    if (r instanceof Error) { if (attempt < 3) { await sleep(3000); continue; } throw r; }
    const t = await r.text();
    if (r.ok) return JSON.parse(t);
    if (attempt < 3 && r.status >= 500) { console.error(`${path}: ${r.status} ${t.slice(0, 300)}; retrying`); await sleep(5000); continue; }
    throw new Error(`${path}: ${r.status} ${t.slice(0, 500)}`);
  }
}

async function config(): Promise<object> {
  const canary = `graft-wp14-canary.${SUB}`, host = `graft-wp14-host.${SUB}`;
  return {
    hostName: `graft-wp14-host.${SUB}`, modelHost: `graft-wp14-model.${SUB}`, registryHost: `graft-wp14-registry.${SUB}`,
    canaryHost: canary, base: "acme/app", pushTarget: "acme-agents/app", other: "acme/secrets",
    prefix: "refs/heads/graftyard/wp14-task/", modelPath: "/v1/chat/completions", knownGoModule: "github.com/graftyard/known",
    canaryV4: (await resolve4(canary))[0], canaryV4All: await resolve4(canary), canaryV6: (await resolve6(canary))[0], hostV4: (await resolve4(host))[0], n: 100,
  };
}

async function hits(n: string, reset = false): Promise<any> {
  const r = await fetch(`${url(n)}/m0/hits${reset ? "/reset" : ""}`, { method: reset ? "POST" : "GET", headers: auth });
  return reset ? r.text() : r.json();
}

async function run(fresh = false): Promise<void> {
  const status = await call("/status").catch(() => ({ running: false }));
  if (fresh || !status.running || !(status.done ?? []).length) for (const n of ["canary", "model", "registry", "host"]) await hits(n, true);
  console.log(JSON.stringify(await call("/run", { method: "POST", body: JSON.stringify({ ...(await config()), fresh }) })));
}

async function wait(): Promise<void> {
  for (;;) {
    const s = await call("/status");
    console.log(new Date().toISOString(), JSON.stringify(s.done), s.alive ? "alive" : "stopped");
    if ((s.done ?? []).length >= 9) break;
    if (!s.alive) { console.log("harness not running; resuming"); await run(); }
    await sleep(15_000);
  }
  await collect();
}

async function collect(): Promise<void> {
  const result = await call("/result");
  const scan = await call("/scan", { method: "POST" });
  const server: Record<string, unknown> = {};
  for (const n of ["canary", "model", "registry", "host"]) server[n] = await hits(n);
  mkdirSync(`${ROOT}results/raw`, { recursive: true });
  writeFileSync(`${ROOT}results/raw/wp14-l0-02.json`, JSON.stringify({ at: new Date().toISOString(), result, scan, server }, null, 1));
  for (const [k, v] of Object.entries(result) as [string, any][]) {
    if (k === "_positives") console.log("positives", JSON.stringify(v));
    else console.log(k.padEnd(20), `${v.succeeded}/${v.attempts}`, JSON.stringify(v.how));
  }
  console.log("scan", JSON.stringify(scan));
  console.log("server", JSON.stringify(server));
}

async function del(): Promise<void> {
  await call("/stop", { method: "POST" }).catch((e) => console.error(String(e)));
  for (const n of [...SCRIPTS].reverse()) {
    try { wrangler(["delete", "--name", `graft-wp14-${n}`, "--force"]); } catch (e) { console.error(`delete graft-wp14-${n}: ${e}`); }
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const cmd = process.argv[2];
if (cmd === "deploy") deploy();
else if (cmd === "run") await run(process.argv.includes("--fresh"));
else if (cmd === "wait") await wait();
else if (cmd === "collect") await collect();
else if (cmd === "delete") await del();
else { console.error("usage: node run/wp14.ts deploy|run|wait|collect|delete"); process.exit(2); }
