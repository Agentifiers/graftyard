// M0-WP2 Raft prototype: deploy and drive. Separate scripts from the graft-site-* bed, so nothing here touches B0-15.
//
//   node run/raft.ts deploy            # graft-raft-a/b/c (two-phase, like deploy.sh) and graft-raft-edge
//   node run/raft.ts status [--group g0]
//   node run/raft.ts op '{"kind":"put","key":"k","value":"v"}' [--group g0]
//
// RAFT_PREFIX=graft-wp4 selects M0-WP4's copy: graft-wp4-a/b/c and graft-wp4-edge, with `SiteHealth`, the WP4 release
// variables, and its own fault plan ("wp4"), so deploying or faulting it never touches B0-11's 48-hour run on
// graft-raft-*.

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { scriptName, SITES, type SiteKey } from "../src/raft/types.ts";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, strict: false, options: { group: { type: "string", default: "g0" } } });
const ROOT = new URL("../", import.meta.url).pathname;
// RAFT_URL and RAFT_TOKEN point the runners at a local `wrangler dev` of src/raft/local.ts instead.
const TOKEN = process.env.RAFT_TOKEN ?? readFileSync(`${ROOT}.m0-token`, "utf8").trim();
export const PREFIX = process.env.RAFT_PREFIX ?? "graft-raft";
const WP4 = PREFIX !== "graft-raft";
export const RAFT_URL = process.env.RAFT_URL ?? `https://${scriptName(PREFIX, "edge")}.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const COMPAT = "2026-10-01";

const bindings = (sites: SiteKey[], from: SiteKey | "edge") => sites.map((k) => ({
  name: `MEMBER_${k.toUpperCase()}`, class_name: "RaftMember", ...(from === k ? {} : { script_name: scriptName(PREFIX, k) }) }));
const faults = { name: "FAULTS", class_name: "FaultControl", script_name: "graft-m0-tools" };
/** The WP4 member variables of release N (1 or more): it reads formats up to `reads` and writes `writes`. */
export interface ReleaseVars { RELEASE: string; READS: string; WRITES: string; BREAK_KIND?: string }
export const RELEASE_1: ReleaseVars = { RELEASE: "1", READS: "1", WRITES: "1" };

export function wrangler(args: string[], input?: string): string {
  return execFileSync("npx", ["wrangler", ...args], { cwd: ROOT, input, encoding: "utf8", stdio: [input ? "pipe" : "ignore", "pipe", "inherit"] });
}

/** Writes site `k`'s member config and returns its path. `phase` "own" binds only the site's own class, for the first
 *  deploy of a new script; "cycle" binds all three. */
export function memberConfig(k: SiteKey, phase: "own" | "cycle", vars: ReleaseVars = RELEASE_1, tag = "cycle"): string {
  const path = `.gen/${PREFIX}-${k}-${tag}.jsonc`;
  const health = { name: "HEALTH", class_name: "SiteHealth" };
  writeFileSync(`${ROOT}${path}`, JSON.stringify({
    name: scriptName(PREFIX, k), main: "../src/raft/member.ts", compatibility_date: COMPAT, workers_dev: false, preview_urls: false,
    vars: WP4 ? { SITE: k, PLAN: "wp4", ...vars } : { SITE: k },
    durable_objects: { bindings: [...bindings(phase === "own" ? [k] : SITES, k), faults, ...(WP4 ? [health] : [])] },
    migrations: [{ tag: "v1", new_sqlite_classes: WP4 ? ["RaftMember", "SiteHealth"] : ["RaftMember"] }],
    observability: { enabled: true },
  }, null, 2));
  return path;
}

function deploy(): void {
  mkdirSync(`${ROOT}.gen`, { recursive: true });
  for (const phase of ["own", "cycle"] as const) for (const k of SITES) wrangler(["deploy", "-c", memberConfig(k, phase, RELEASE_1, phase)]);
  const edge = `.gen/${PREFIX}-edge.jsonc`;
  writeFileSync(`${ROOT}${edge}`, JSON.stringify({
    name: scriptName(PREFIX, "edge"), main: "../src/raft/edge.ts", compatibility_date: COMPAT, workers_dev: true, preview_urls: false,
    ...(WP4 ? { vars: { PLAN: "wp4" } } : {}),
    durable_objects: { bindings: [...bindings(SITES, "edge"), faults,
      ...(WP4 ? SITES.map((k) => ({ name: `HEALTH_${k.toUpperCase()}`, class_name: "SiteHealth", script_name: scriptName(PREFIX, k) })) : [])] },
    observability: { enabled: true },
  }, null, 2));
  wrangler(["deploy", "-c", edge]);
  wrangler(["secret", "put", "M0_TOKEN", "-c", edge], TOKEN);
}

export async function raft<T>(path: string, body: unknown = {}, timeoutMs = 120_000): Promise<T> {
  const res = await fetch(RAFT_URL + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` },
    signal: AbortSignal.timeout(timeoutMs) });
  const text = await res.text();
  if (!res.ok) throw new Error(`${path}: ${res.status} ${text.slice(0, 300)}`);
  return JSON.parse(text) as T;
}

/** Creates `group` and moves its leadership to site A, the group's `home`, by transfer, as bootstrap does
 *  (design/DESIGN.md, `bootstrap`): A's first campaign can miss its deadline while the other members cold-start. */
export async function createLedFromA(group: string): Promise<void> {
  await raft("/op", { group, op: { kind: "put", key: "warm", value: "1" } });
  for (let i = 0; i < 10; i++) {
    const st = await raft<{ site: string; role: string }[]>("/status", { group });
    const leader = st.find((s) => s.role === "leader")?.site;
    if (leader === "a") return;
    if (leader) await raft("/transfer", { group, to: "a" }).catch(() => {});
    else await raft("/op", { group, op: { kind: "put", key: "warm", value: "1" } }).catch(() => {});
    await new Promise((r) => setTimeout(r, 1_000));
  }
  throw new Error(`could not move ${group}'s leadership to site a`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [cmd, arg] = positionals;
  switch (cmd) {
    case "deploy":
      if (!WP4 && !process.argv.includes("--yes-reset-48h")) throw new Error("graft-raft-* runs B0-11's 48-hour run; deploying resets it. Pass --yes-reset-48h, or set RAFT_PREFIX=graft-wp4");
      deploy(); break;
    case "status": console.log(JSON.stringify(await raft("/status", { group: opt.group }), null, 1)); break;
    case "op": console.log(JSON.stringify(await raft("/op", { group: opt.group, op: JSON.parse(arg) }))); break;
    default: throw new Error("usage: node run/raft.ts deploy|status|op");
  }
}
