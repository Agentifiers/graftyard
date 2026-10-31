// M0-WP10 driver library: real `git` against the host's smart HTTP, REST and GraphQL calls, local object building
// with git plumbing, and token minting with the harness secret. Used by run/agent-b036.ts and run/agent-b037.ts.

import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { mint } from "../src/agent/token.ts";
import { CREDS } from "../src/agent/fixture.ts";

export const ROOT = new URL("../", import.meta.url).pathname;
export const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
export const SUBDOMAIN = "<workers-subdomain>";
export const REMOTE = `https://graft-m0-agent.${SUBDOMAIN}.workers.dev`;

export interface Run { code: number; out: string; err: string; ms: number }

export function sh(cmd: string, args: string[], opt: { cwd?: string; env?: Record<string, string>; input?: string | Uint8Array } = {}): Promise<Run> {
  const t = Date.now();
  return new Promise((res) => {
    const p = spawn(cmd, args, { cwd: opt.cwd, env: { ...process.env, GIT_TERMINAL_PROMPT: "0", ...opt.env } });
    const out: Buffer[] = [], err: Buffer[] = [];
    p.stdout.on("data", (b) => out.push(b));
    p.stderr.on("data", (b) => err.push(b));
    p.on("close", (code) => res({ code: code ?? -1, out: Buffer.concat(out).toString("utf8"), err: Buffer.concat(err).toString("utf8"), ms: Date.now() - t }));
    if (opt.input != null) p.stdin.end(opt.input); else p.stdin.end();
  });
}

const tokens = new Map<string, string>();
export async function tok(label: string): Promise<string> {
  if (!tokens.has(label)) tokens.set(label, await mint(TOKEN, CREDS[label]));
  return tokens.get(label)!;
}

/** A local bare repository for building objects with plumbing. */
export class Work {
  dir: string;
  constructor(name: string) {
    this.dir = join(ROOT, ".gen", `wp10-work-${name}.git`);
    if (!existsSync(this.dir)) { mkdirSync(this.dir, { recursive: true }); }
  }
  async init(): Promise<void> {
    rmSync(this.dir, { recursive: true, force: true });
    mkdirSync(this.dir, { recursive: true });
    await this.git(["init", "-q", "--bare", "-b", "main"]);
  }
  async git(args: string[], input?: string | Uint8Array, env: Record<string, string> = {}): Promise<string> {
    const r = await sh("git", ["--git-dir", this.dir, ...args], { input, env: { GIT_AUTHOR_NAME: "m0", GIT_AUTHOR_EMAIL: "m0@example.com",
      GIT_COMMITTER_NAME: "m0", GIT_COMMITTER_EMAIL: "m0@example.com", GIT_AUTHOR_DATE: "1700000000 +0000", GIT_COMMITTER_DATE: "1700000000 +0000", ...env } });
    if (r.code !== 0) throw new Error(`git ${args.join(" ")}: ${r.err}`);
    return r.out.trim();
  }
  /** A tree: `parent`'s tree (or empty) with `files` written (null deletes). */
  async tree(parent: string | null, files: Record<string, string | null>): Promise<string> {
    const idx = mkdtempSync(join(tmpdir(), "wp10-idx-"));
    const env = { GIT_INDEX_FILE: join(idx, "index") };
    try {
      if (parent) await this.git(["read-tree", `${parent}^{tree}`], undefined, env); else await this.git(["read-tree", "--empty"], undefined, env);
      for (const [p, c] of Object.entries(files)) {
        if (c == null) { await this.git(["update-index", "--force-remove", p], undefined, env); continue; }
        const oid = await this.git(["hash-object", "-w", "--stdin"], c);
        await this.git(["update-index", "--add", "--cacheinfo", `100644,${oid},${p}`], undefined, env);
      }
      return await this.git(["write-tree"], undefined, env);
    } finally { rmSync(idx, { recursive: true, force: true }); }
  }
  async commit(parent: string | null, files: Record<string, string | null>, message: string): Promise<string> {
    const tree = await this.tree(parent, files);
    return this.git(["commit-tree", tree, ...(parent ? ["-p", parent] : [])], message);
  }
  /** A commit object written byte for byte (malformed messages, odd headers), as `git hash-object --literally`. */
  async rawCommit(tree: string, parent: string | null, headerExtra: string, message: Uint8Array | string): Promise<string> {
    const head = `tree ${tree}\n${parent ? `parent ${parent}\n` : ""}author m0 <m0@example.com> 1700000000 +0000\ncommitter m0 <m0@example.com> 1700000000 +0000\n${headerExtra}\n`;
    const msg = typeof message === "string" ? Buffer.from(message, "utf8") : Buffer.from(message);
    return this.git(["hash-object", "-t", "commit", "-w", "--literally", "--stdin"], Buffer.concat([Buffer.from(head, "utf8"), msg]));
  }
  /** A tree from raw entries at any depth and mode (gitlinks included), through a scratch index. */
  async treeEntries(entries: { mode: string; oid: string; path: string }[]): Promise<string> {
    const idx = mkdtempSync(join(tmpdir(), "wp10-idx-"));
    const env = { GIT_INDEX_FILE: join(idx, "index") };
    try {
      await this.git(["read-tree", "--empty"], undefined, env);
      if (entries.length) await this.git(["update-index", "--add", "--index-info"], entries.map((e) => `${e.mode} ${e.oid}\t${e.path}\n`).join(""), env);
      return await this.git(["write-tree"], undefined, env);
    } finally { rmSync(idx, { recursive: true, force: true }); }
  }
  async fetchFrom(url: string, token: string, refspec: string): Promise<Run> {
    return sh("git", ["--git-dir", this.dir, ...authArgs(token), "fetch", "-q", url, refspec]);
  }
}

export function authArgs(token: string, headers: Record<string, string> = {}): string[] {
  const a = ["-c", "credential.helper=", "-c", `http.extraHeader=Authorization: Basic ${Buffer.from(`x-access-token:${token}`).toString("base64")}`];
  for (const [k, v] of Object.entries(headers)) a.push("-c", `http.extraHeader=${k}: ${v}`);
  return a;
}

export interface PushResult { code: number; refs: { ref: string; flag: string; summary: string }[]; remote: string[]; err: string; ms: number }

/** `git push --porcelain` from a Work repository. */
export async function push(work: Work, url: string, token: string, refspecs: string[], opt: { headers?: Record<string, string>; atomic?: boolean; probe?: string; proto?: number } = {}): Promise<PushResult> {
  const headers = { ...(opt.headers ?? {}), ...(opt.probe ? { "X-M0-Probe": opt.probe } : {}) };
  const args = ["--git-dir", work.dir, ...authArgs(token, headers), "-c", `protocol.version=${opt.proto ?? 2}`, "-c", "http.lowSpeedLimit=1", "-c", "http.lowSpeedTime=60", "-c", "http.connectTimeout=30",
    "push", "--porcelain", ...(opt.atomic ? ["--atomic"] : []), url, ...refspecs];
  let r = await sh("git", args);
  // No report-status at all (HTTP 5xx or a dropped connection during an object reset): nothing was decided, resend.
  for (let i = 0; i < 4 && r.code !== 0 && !/^[ +\-*=!]\t/m.test(r.out) && /RPC failed|HTTP 5\d\d|unexpected disconnect|Operation too slow|returned error: 5|unable to access|Connection timed out/.test(r.err); i++) {
    await new Promise((res) => setTimeout(res, 1000 * (i + 1)));
    r = await sh("git", args);
  }
  const refs = r.out.split("\n").filter((l) => /^[ +\-*=!]\t/.test(l)).map((l) => { const [flag, ref, summary] = l.split("\t"); return { flag, ref, summary }; });
  const remote = r.err.split("\n").filter((l) => l.startsWith("remote:")).map((l) => l.slice(7).trim());
  return { code: r.code, refs, remote, err: r.err.trim().slice(-600), ms: r.ms };
}

export async function api(base: string, method: string, path: string, token: string, body?: unknown, headers: Record<string, string> = {}): Promise<{ status: number; body: any; ms: number }> {
  const t = Date.now();
  for (let i = 0; ; i++) {
    try {
      const r = await fetch(`${base}${path}`, { method, headers: { authorization: `Bearer ${token}`, "content-type": "application/json", accept: "application/vnd.github+json", ...headers },
        body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(60_000) });
      const text = await r.text();
      // A Durable Object reset answers 500/503 or drops the request; a refused write leaves nothing, so resend.
      if ((r.status >= 500) && i < 4) { await new Promise((res) => setTimeout(res, 1000 * (i + 1))); continue; }
      let j: any = text;
      try { j = text ? JSON.parse(text) : null; } catch { /* not JSON */ }
      return { status: r.status, body: j, ms: Date.now() - t };
    } catch (e) {
      if (i >= 4) throw e;
      await new Promise((r) => setTimeout(r, 500 * (i + 1)));
    }
  }
}

export async function gql(base: string, token: string, mutation: string, input: unknown, probe?: string): Promise<{ status: number; body: any; ms: number }> {
  return api(base, "POST", "/api/graphql", token, { query: `mutation($input: ${mutation[0].toUpperCase()}${mutation.slice(1)}Input!) { ${mutation}(input: $input) { clientMutationId } }`, variables: { input } },
    probe ? { "x-m0-probe": probe } : {});
}

export async function harness(base: string, path: string, body?: unknown): Promise<any> {
  for (let i = 0; ; i++) {
    try {
      const r = await fetch(`${base}${path}`, { method: body === undefined ? "GET" : "POST", headers: { authorization: `Bearer ${TOKEN}` }, body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(120_000) });
      if (!r.ok) throw new Error(`${path}: ${r.status} ${await r.text()}`);
      return r.json();
    } catch (e) {
      if (i >= 5) throw e;
      await new Promise((r) => setTimeout(r, 1000 * (i + 1)));
    }
  }
}

/** Runs `n` tasks with at most `k` in flight. */
export async function pool<T>(n: number, k: number, f: (i: number) => Promise<T>): Promise<T[]> {
  const out: T[] = new Array(n);
  let next = 0;
  await Promise.all(Array.from({ length: Math.min(k, n) }, async () => { while (next < n) { const i = next++; out[i] = await f(i); } }));
  return out;
}

export const baseUrl = (opt: { url?: string }) => (opt.url ?? REMOTE).replace(/\/$/, "");
