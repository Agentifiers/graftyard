// M0-WP8 (B0-28): deploy and drive the restore-window prototype in src/wp8/. Its own scripts graft-wp8-a/b/c and
// graft-wp8-edge and its own buckets graft-wp8-a/b/c, so nothing here touches graft-raft-*, graft-wp4-*, or the tools.
//
//   node run/wp8.ts deploy
//   node run/wp8-replay.ts b028 [--group b028-1]   the B0-28 replay, day -40 to day 30, then the restore
//   node run/wp8-replay.ts neg  [--group neg-1]    followers refuse to truncate past an un-Archived range
//
// Git objects are real: each push runs `git` in a scratch repository on the runner and posts every new object's type and
// raw body (`git cat-file`), which the edge checks against its SHA-1.

import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { bucketName, DAY, HINT, scriptName, SITES, type SiteKey } from "../src/wp8/types.ts";

const ROOT = new URL("../", import.meta.url).pathname;
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const URL_ = `https://${scriptName("edge")}.<workers-subdomain>.workers.dev`;
const COMPAT = "2026-10-01";

export function wrangler(args: string[], input?: string): string {
  return execFileSync("npx", ["wrangler", ...args], { cwd: ROOT, input, encoding: "utf8", stdio: [input ? "pipe" : "ignore", "pipe", "pipe"] });
}
const buckets = SITES.map((k) => ({ binding: `BUCKET_${k.toUpperCase()}`, bucket_name: bucketName(k) }));
const members = (from: SiteKey | "edge", sites: SiteKey[]) => sites.map((k) => ({ name: `MEMBER_${k.toUpperCase()}`, class_name: "Wp8Member",
  ...(from === k ? {} : { script_name: scriptName(k) }) }));

function deploy(): void {
  mkdirSync(`${ROOT}.gen`, { recursive: true });
  // `wrangler r2 bucket list` failed intermittently on this account (code 10042), so the check uses the REST API.
  const api = `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/r2/buckets?name_contains=graft-wp8`;
  const have = execFileSync("curl", ["-s", "-H", `Authorization: Bearer ${process.env.CLOUDFLARE_API_TOKEN}`, api], { encoding: "utf8" });
  for (const k of SITES) if (!have.includes(`"${bucketName(k)}"`)) wrangler(["r2", "bucket", "create", bucketName(k), "--location", HINT[k]]);
  for (const phase of ["own", "cycle"] as const) for (const k of SITES) {
    const path = `.gen/wp8-${k}-${phase}.jsonc`;
    writeFileSync(`${ROOT}${path}`, JSON.stringify({
      name: scriptName(k), main: "../src/wp8/member.ts", compatibility_date: COMPAT, workers_dev: false, preview_urls: false,
      vars: { SITE: k }, r2_buckets: buckets, durable_objects: { bindings: members(k, phase === "own" ? [k] : SITES) },
      migrations: [{ tag: "v1", new_sqlite_classes: ["Wp8Member"] }], observability: { enabled: true },
    }, null, 2));
    wrangler(["deploy", "-c", path]);
  }
  const edge = ".gen/wp8-edge.jsonc";
  writeFileSync(`${ROOT}${edge}`, JSON.stringify({
    name: scriptName("edge"), main: "../src/wp8/edge.ts", compatibility_date: COMPAT, workers_dev: true, preview_urls: false,
    r2_buckets: buckets, durable_objects: { bindings: members("edge", SITES) }, observability: { enabled: true },
  }, null, 2));
  wrangler(["deploy", "-c", edge]);
  wrangler(["secret", "put", "M0_TOKEN", "-c", edge], TOKEN);
}

export async function edge<T = any>(path: string, body: Record<string, unknown>, timeoutMs = 120_000): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    const res = await fetch(URL_ + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(timeoutMs) })
      .catch((e) => e as Error);
    if (res instanceof Error) { if (attempt < 2) continue; throw res; }
    const text = await res.text();
    if (res.ok) return JSON.parse(text) as T;
    if (attempt < 2 && res.status >= 500 && !text.includes('"error"')) continue;
    throw new Error(`${path}: ${res.status} ${text.slice(0, 400)}`);
  }
}

// ---- real git on the runner ----

export class Repo {
  readonly dir = mkdtempSync(`${tmpdir()}/wp8-`);
  private sent = new Set<string>();
  constructor() { this.git("init", "-q", "-b", "main"); }
  git(...args: string[]): string {
    return execFileSync("git", args, { cwd: this.dir, encoding: "utf8", env: { ...process.env, GIT_AUTHOR_NAME: "wp8", GIT_AUTHOR_EMAIL: "wp8@example.com",
      GIT_COMMITTER_NAME: "wp8", GIT_COMMITTER_EMAIL: "wp8@example.com", GIT_AUTHOR_DATE: "2026-10-01T00:00:00Z", GIT_COMMITTER_DATE: "2026-10-01T00:00:00Z" } }).trim();
  }
  /** A commit on `parent` (or a root commit) that writes `files`, with `bytes` of random content in each. */
  commit(parent: string | null, msg: string, files: string[], bytes: number): string {
    this.git("clean", "-fdq");
    if (parent) this.git("checkout", "-qf", "--detach", parent);
    else { this.git("checkout", "-q", "--orphan", `o${Math.random().toString(36).slice(2)}`); this.git("rm", "-rqf", "--ignore-unmatch", "."); }
    for (const f of files) {
      mkdirSync(`${this.dir}/${f.split("/").slice(0, -1).join("/") || "."}`, { recursive: true });
      writeFileSync(`${this.dir}/${f}`, `${msg} ${f}\n${Buffer.from(crypto.getRandomValues(new Uint8Array(bytes))).toString("base64")}\n`);
    }
    this.git("add", ...files);
    this.git("commit", "-q", "-m", msg);
    return this.git("rev-parse", "HEAD");
  }
  /** Every object reachable from `tip` not sent before, as type and raw body (base64). */
  objects(tip: string): { oid: string; type: string; data: string }[] {
    const oids = this.git("rev-list", "--objects", tip).split("\n").map((l) => l.split(" ")[0]).filter((o) => o && !this.sent.has(o));
    return oids.map((oid) => {
      this.sent.add(oid);
      const type = this.git("cat-file", "-t", oid);
      const data = execFileSync("git", ["cat-file", type, oid], { cwd: this.dir }).toString("base64");
      return { oid, type, data };
    });
  }
}

export const iso = (t: number) => new Date(t).toISOString().slice(0, 16);
export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
export function save(name: string, v: unknown): void {
  mkdirSync(`${ROOT}results/raw`, { recursive: true });
  writeFileSync(`${ROOT}results/raw/${name}`, JSON.stringify(v, null, 1));
}
export { DAY };

if (import.meta.url === `file://${process.argv[1]}`) {
  const cmd = process.argv[2];
  if (cmd === "deploy") deploy();
  else throw new Error("usage: node run/wp8.ts deploy (the replays are run/wp8-replay.ts b028|neg)");
}
