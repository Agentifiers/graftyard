// graft-m0-tools: deploy, place colo-pinned load clients, run load, collect histories, and check them with Porcupine
// (build/BUILD-PLAN.md, M0-WP1 tools). Its own script, so none of this creates a site-script version.
//
//   node run/tools.ts deploy
//   node run/tools.ts place --colos ATL,LHR,SIN [--per 1] [--tries 40]   # pins clients; writes results/raw/tools-clients.json
//   node run/tools.ts faults '<FaultPlan JSON>'                          # sets the plan; '{"rules":[]}' clears it
//   node run/tools.ts load --target REGISTER --object r1 --seconds 60 [--concurrency 4 --rate 5 --keys 8 --read 0.5]
//   node run/tools.ts load --target RAFT --service --object <group> ...   # M0-WP2: through graft-raft-edge
//   node run/tools.ts collect --run <run>                                # writes results/raw/history-<run>.jsonl
//   node run/tools.ts check --run <run>                                  # runs porcupine/ on that file
//   node run/tools.ts selftest                                           # results/tools-selftest.json

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  colos: { type: "string" }, per: { type: "string", default: "1" }, tries: { type: "string", default: "40" },
  target: { type: "string", default: "REGISTER" }, object: { type: "string", default: "r1" },
  seconds: { type: "string", default: "60" }, concurrency: { type: "string", default: "4" }, rate: { type: "string", default: "5" },
  keys: { type: "string", default: "8" }, read: { type: "string", default: "0.5" }, service: { type: "boolean", default: false },
  run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) },
} });
const [cmd, arg] = positionals;
const ROOT = new URL("../", import.meta.url).pathname;
const RAW = `${ROOT}results/raw/`;
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const URL_ = `https://graft-m0-tools.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const CLIENTS = `${RAW}tools-clients.json`;
mkdirSync(RAW, { recursive: true });

// Colo to region hint, for the colos M0-WP1 saw Durable Objects land in (results/b0-12.json, memberColos).
const HINT_OF: Record<string, string> = { ATL: "enam", EWR: "enam", IAD: "enam", ORD: "enam", MIA: "enam",
  LHR: "weur", AMS: "weur", CDG: "weur", MAD: "weur", LIS: "weur", MRS: "weur", SIN: "apac", NRT: "apac", KIX: "apac",
  HKG: "apac", ICN: "apac" };

async function call<T>(path: string, body?: unknown): Promise<T> {
  for (let attempt = 1; ; attempt++) {
    let err: string;
    try {
      const res = await fetch(URL_ + path, { method: body === undefined ? "GET" : "POST", body: body === undefined ? undefined : JSON.stringify(body),
        headers: { authorization: `Bearer ${TOKEN}` } });
      if (res.ok) return res.json() as Promise<T>;
      err = `${res.status} ${await res.text()}`;
    } catch (e) { err = String(e); }
    if (attempt >= 5) throw new Error(`${path}: ${err}`);
    await new Promise((r) => setTimeout(r, 2000 * attempt));
  }
}

function deploy(): void {
  mkdirSync(`${ROOT}.gen`, { recursive: true });
  writeFileSync(`${ROOT}.gen/tools.jsonc`, JSON.stringify({
    name: "graft-m0-tools", main: "../src/tools.ts", compatibility_date: "2026-10-01", workers_dev: true, preview_urls: false,
    compatibility_flags: ["global_fetch_strictly_public"],
    // The M0-WP2 Raft edge, so pinned load clients drive B0-11 from three colos with `service: true`.
    services: [{ binding: "RAFT", service: "graft-raft-edge", entrypoint: "RaftEdge" }],
    durable_objects: { bindings: [
      { name: "FAULTS", class_name: "FaultControl" }, { name: "CLIENT", class_name: "LoadClient" },
      { name: "REGISTER", class_name: "Register" }, { name: "STALE_REGISTER", class_name: "StaleRegister" },
      { name: "QUORUM", class_name: "QuorumProbe" }, { name: "ROUTE", class_name: "RouteProbe" }] },
    migrations: [{ tag: "v1", new_sqlite_classes: ["FaultControl", "LoadClient", "Register", "StaleRegister"] },
      { tag: "v2", new_sqlite_classes: ["QuorumProbe"] }, { tag: "v3", new_sqlite_classes: ["RouteProbe"] }],
    observability: { enabled: true },
  }, null, 2));
  execFileSync("npx", ["wrangler", "deploy", "-c", ".gen/tools.jsonc"], { cwd: ROOT, stdio: ["ignore", "ignore", "inherit"] });
  execFileSync("npx", ["wrangler", "secret", "put", "M0_TOKEN", "-c", ".gen/tools.jsonc"], { cwd: ROOT, input: TOKEN, stdio: ["pipe", "ignore", "inherit"] });
}

type Pinned = Record<string, string[]>; // colo -> client names
const pinned = (): Pinned => (existsSync(CLIENTS) ? JSON.parse(readFileSync(CLIENTS, "utf8")) : {});

/** Creates clients with the colo's hint until `per` land in that colo. A client never moves, so it stays pinned.
 *  Misses are left idle; an idle Durable Object bills nothing. */
async function place(colos: string[], per: number, tries: number): Promise<Pinned> {
  const out = pinned();
  const stamp = Date.now().toString(36);
  for (const c of colos) {
    const hint = HINT_OF[c];
    if (!hint) throw new Error(`no hint known for colo ${c}`);
    out[c] ??= [];
    for (let batch = 0; out[c].length < per && batch * 10 < tries; batch++) {
      const names = Array.from({ length: 10 }, (_, i) => `client-${c}-${stamp}-${batch}-${i}`);
      const landed = await call<(string | { error: string })[]>("/clients/place", { hint, names });
      names.forEach((n, i) => { if (landed[i] === c && out[c].length < per) out[c].push(n); });
    }
    console.error(`${c}: ${out[c].length} of ${per} pinned`);
  }
  writeFileSync(CLIENTS, JSON.stringify(out, null, 2));
  return out;
}

async function load(run: string, names: string[], target: string, object: string, seconds: number): Promise<void> {
  const spec = { run, target, object, concurrency: Number(opt.concurrency), rate: Number(opt.rate), durationMs: seconds * 1000,
    keys: Number(opt.keys), readShare: Number(opt.read), service: opt.service, callTimeoutMs: opt.service ? 45_000 : undefined };
  console.error(JSON.stringify(await call("/clients/start", { names, spec })));
  const end = Date.now() + seconds * 1000;
  while (Date.now() < end + 5000) {
    await new Promise((r) => setTimeout(r, 10_000));
    const st = await call<{ running: boolean; rows: number; failed: number; colo: string }[]>("/clients/status", { names });
    console.error(st.map((s) => `${s.colo} ${s.rows}/${s.failed}${s.running ? "" : " done"}`).join("  "));
    if (Date.now() > end && st.every((s) => !s.running)) break;
  }
}

async function collect(run: string, names: string[]): Promise<string> {
  const path = `${RAW}history-${run}.jsonl`;
  const lines: string[] = [];
  for (const name of names) {
    for (let after = 0; ;) {
      const page = await call<{ seq: number }[]>("/clients/history", { name, run, after });
      if (!page.length) break;
      lines.push(...page.map((r) => JSON.stringify(r)));
      after = page[page.length - 1].seq;
    }
  }
  writeFileSync(path, lines.join("\n") + "\n");
  console.error(`${lines.length} operations -> ${path}`);
  return path;
}

function check(path: string, viz?: string): { result: string; operations: number; noReplyWrites: number; droppedFailedReads: number } {
  const args = ["run", ".", ...(viz ? ["-viz", viz] : []), path];
  try {
    return JSON.parse(execFileSync("go", args, { cwd: `${ROOT}porcupine`, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] }));
  } catch (e) {
    const out = (e as { stdout?: string }).stdout;
    if (out) return JSON.parse(out); // exit 1 means not linearizable, and the summary is still printed
    throw e;
  }
}

const names = () => Object.values(pinned()).flat();

switch (cmd) {
  case "deploy": deploy(); break;
  case "place": await place(opt.colos!.split(","), Number(opt.per), Number(opt.tries)); break;
  case "faults": console.log(JSON.stringify(await call("/faults", JSON.parse(arg ?? '{"rules":[]}')))); break;
  case "load": await load(opt.run!, names(), opt.target!, opt.object!, Number(opt.seconds)); console.log(opt.run); break;
  case "collect": await collect(opt.run!, names()); break;
  case "check": console.log(JSON.stringify(check(`${RAW}history-${opt.run}.jsonl`))); break;
  case "selftest": {
    // Proves each tool against Cloudflare: clients pinned to one colo per Tri region, the checker on a correct register
    // with no faults, on the same register with 20% of member calls failing and 20% of writes resetting the object,
    // and on a register that serves stale reads, which it must reject.
    const pins = await place(["ATL", "LHR", "SIN"], 1, 60);
    const all = Object.values(pins).flat();
    const seconds = Number(opt.seconds);
    const cases = [
      { name: "clean", target: "REGISTER", plan: { rules: [] }, expect: "Ok" },
      { name: "faults", target: "REGISTER", plan: { rules: [{ target: "call", mode: "error", rate: 0.1 },
        { target: "call", mode: "drop-reply", rate: 0.1 }, { target: "write", rate: 0.2 }] }, expect: "Ok" },
      { name: "stale", target: "STALE_REGISTER", plan: { rules: [] }, expect: "Illegal" },
    ];
    const results = [];
    for (const c of cases) {
      const run = `${opt.run}-${c.name}`;
      await call("/faults", { plan: c.plan });
      await call("/register/reset", { object: run });
      await load(run, all, c.target, run, seconds);
      const history = await collect(run, all);
      const rows = readFileSync(history, "utf8").trim().split("\n").map((l) => JSON.parse(l) as { colo: string; ok: boolean; error?: string });
      const errors: Record<string, number> = {};
      for (const r of rows) if (!r.ok) {
        const key = (r.error ?? "").replace(/(STALE_)?REGISTER:op|register:\w+/g, "*").slice(0, 60);
        errors[key] = (errors[key] ?? 0) + 1;
      }
      results.push({ case: c.name, plan: c.plan, expect: c.expect, ...check(history),
        clientColos: [...new Set(rows.map((r) => r.colo))], failed: rows.filter((r) => !r.ok).length, errors });
      console.error(JSON.stringify(results.at(-1)));
    }
    await call("/faults", { plan: { rules: [] } });
    const summary = { at: new Date().toISOString(), run: opt.run, seconds, pinned: pins,
      verdict: results.every((r) => r.result === r.expect) ? "Pass" : "Fail", results };
    writeFileSync(`${ROOT}results/tools-selftest.json`, JSON.stringify(summary, null, 2));
    console.log(JSON.stringify(summary, null, 2));
    break;
  }
  default: throw new Error("usage: node run/tools.ts deploy|place|faults|load|collect|check|selftest");
}
