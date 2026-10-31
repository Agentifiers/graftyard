// B0-35 (build/BUILD-PLAN.md, M0-WP9): lazy test merges under TEST_MERGE_BUDGET, with real `git merge-tree` on a
// synthetic monorepo in this orb. The scheduler follows design/DESIGN.md, Shape, `TestMerge`: one compute per (PR, base
// tip); computes run on a head push, on Actions dispatch for each `pull_request`-family event, and on a single-PR GET
// by a writer or CI token at most once per PR per 30 s; list fields never compute; a budget of TEST_MERGE_BUDGET of the
// k shards' vCPU serves pushes and dispatch first, then GETs; over it `mergeable` is null.
//
//   node run/wp9-testmerge.ts setup [--files 100000]       # builds /tmp/wp9-mono (bare) with 500 PR branches
//   node run/wp9-testmerge.ts run [--seconds 300] [--k 7] [--run X]
//
// Rates are journey 9's: 500 agents push their PR head every 2 min (4.17/s), the base moves 10 times a minute, 500
// clients GET their PR every 5 s (100/s), and 10 list requests/s. vCPU is the git children's user + sys time, read
// with the shell's `times`. The budget is TEST_MERGE_BUDGET x 4 vCPU x k as CPU-seconds a second, with at most
// ceil(budget) merges at once. Every head push queues one `synchronize` dispatch, which needs the test merge for its
// head against the base tip at compute time; push and dispatch for one head coalesce into one compute.

import { execFile, execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  files: { type: "string", default: "100000" }, seconds: { type: "string", default: "300" }, k: { type: "string", default: "7" },
  run: { type: "string", default: new Date().toISOString().replace(/[-:.]/g, "").slice(0, 13) }, prs: { type: "string", default: "500" },
  /** Files each head push and each base move changes (3 in journey 9's shape). */
  touch: { type: "string", default: "3" },
  /** Outside the design: a budget in vCPU instead of TEST_MERGE_BUDGET x 4 x k, to drive the over-budget path. */
  budget: { type: "string" } } });
const TOUCH = Number(opt.touch);
const ROOT = new URL("../", import.meta.url).pathname;
const REPO = "/tmp/wp9-mono";
const PRS = Number(opt.prs);
const TEST_MERGE_BUDGET = 0.25, SHARD_VCPU = 4, GET_COALESCE_MS = 30_000;
const PUSH_EVERY_S = 120, BASE_MOVES_PER_MIN = 10, POLL_EVERY_S = 5, LISTS_PER_S = 10;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const git = (args: string[], input?: string) => execFileSync("git", ["-C", REPO, ...args], { input, encoding: "utf8", maxBuffer: 1 << 30 }).trim();
const gitAsync = (args: string[], env: Record<string, string> = {}) => new Promise<string>((resolve, reject) =>
  execFile("git", ["-C", REPO, ...args], { encoding: "utf8", env: { ...process.env, ...env }, maxBuffer: 1 << 26 }, (e, out) => (e ? reject(e) : resolve(out.trim()))));

function setup(): void {
  const files = Number(opt.files);
  rmSync(REPO, { recursive: true, force: true });
  execFileSync("git", ["init", "--bare", "-q", REPO]);
  // fast-import: a first commit of `files` text files in 1,000 directories, then one commit per PR changing 3 files in
  // its own package (pkg/<pr % 50>/), each on refs/heads/pr/<n>, branched from main.
  const lines: string[] = [];
  const rnd = (n: number) => Array.from({ length: n }, () => Math.random().toString(36).slice(2)).join("\n");
  lines.push("commit refs/heads/main", "mark :1", "committer m0 <m0@x> 1700000000 +0000", "data 4", "base");
  for (let i = 0; i < files; i++) {
    const body = `${i}\n${rnd(40)}\n`;
    lines.push(`M 100644 inline pkg/${i % 50}/d${Math.floor(i / 50) % 20}/f${i}.txt`, `data ${Buffer.byteLength(body)}`, body);
  }
  for (let p = 0; p < PRS; p++) {
    lines.push(`commit refs/heads/pr/${p}`, "committer m0 <m0@x> 1700000001 +0000", "data 3", "pr", "from :1");
    for (let j = 0; j < 3; j++) {
      const f = (p % 50) + 50 * ((p * 7 + j * 131) % Math.floor(files / 50));
      const body = `${f}\npr ${p}\n${rnd(40)}\n`;
      lines.push(`M 100644 inline pkg/${f % 50}/d${Math.floor(f / 50) % 20}/f${f}.txt`, `data ${Buffer.byteLength(body)}`, body);
    }
  }
  execFileSync("git", ["-C", REPO, "fast-import", "--quiet"], { input: lines.join("\n") + "\n", maxBuffer: 1 << 30 });
  execFileSync("git", ["-C", REPO, "repack", "-adq"]);
  console.log(JSON.stringify({ repo: REPO, files, prs: PRS, size: execFileSync("du", ["-sh", REPO], { encoding: "utf8" }).trim() }));
}

/** A new commit on `parent` changing `paths` (new random contents), through a temporary index. Returns the commit. */
async function commitOn(parent: string, paths: string[], msg: string): Promise<string> {
  const idx = `/tmp/wp9-idx-${process.pid}-${Math.random().toString(36).slice(2)}`;
  const env = { GIT_INDEX_FILE: idx };
  try {
    await gitAsync(["read-tree", parent], env);
    const infos: string[] = [];
    for (const p of paths) {
      const blob = await new Promise<string>((resolve, reject) => {
        const c = execFile("git", ["-C", REPO, "hash-object", "-w", "--stdin"], (e, out) => (e ? reject(e) : resolve(out.trim())));
        c.stdin!.end(`${p}\n${msg}\n${Math.random()}\n`);
      });
      infos.push(`100644 ${blob}\t${p}`);
    }
    await new Promise<void>((resolve, reject) => {
      const c = execFile("git", ["-C", REPO, "update-index", "--index-info"], { env: { ...process.env, ...env } }, (e) => (e ? reject(e) : resolve()));
      c.stdin!.end(infos.join("\n") + "\n");
    });
    const tree = await gitAsync(["write-tree"], env);
    return await gitAsync(["commit-tree", tree, "-p", parent, "-m", msg]);
  } finally { rmSync(idx, { force: true }); }
}

/** One test merge with real git; returns its children's CPU seconds and whether it conflicted. */
function mergeTree(base: string, head: string): Promise<{ cpu: number; conflict: boolean; wallMs: number }> {
  const t = Date.now();
  return new Promise((resolve) => {
    execFile("bash", ["-c", `git -C ${REPO} merge-tree --write-tree ${base} ${head} >/dev/null 2>&1; rc=$?; times; echo $rc`],
      { encoding: "utf8" }, (_e, out) => {
        const ls = out.trim().split("\n");
        const child = ls[1].split(/\s+/).map((x) => { const m = /(\d+)m([\d.]+)s/.exec(x)!; return Number(m[1]) * 60 + Number(m[2]); });
        resolve({ cpu: child[0] + child[1], conflict: ls[2] !== "0", wallMs: Date.now() - t });
      });
  });
}

type Job = { pr: number; head: string; src: "push" | "get"; at: number; dispatchAt?: number };

async function run(): Promise<void> {
  if (!existsSync(REPO)) throw new Error("run setup first");
  const seconds = Number(opt.seconds), k = Number(opt.k);
  const budget = opt.budget ? Number(opt.budget) : TEST_MERGE_BUDGET * SHARD_VCPU * k; // vCPU
  const slots = Math.ceil(budget);
  const mainStart = git(["rev-parse", "refs/heads/main"]);
  const files = Number(git(["ls-tree", "-r", "--name-only", mainStart]).split("\n").length);
  const prPaths = (p: number, n: number) => Array.from({ length: TOUCH }, (_, j) => {
    const f = (p % 50) + 50 * ((p * 7 + j * 131 + n * 17) % Math.floor(files / 50));
    return `pkg/${f % 50}/d${Math.floor(f / 50) % 20}/f${f}.txt`;
  });
  let base = mainStart;
  const heads = Array.from({ length: PRS }, (_, p) => git(["rev-parse", `refs/heads/pr/${p}`]));
  /** (PR) -> the (head, base) its current test merge is for, and when a GET last computed it. */
  const current = new Map<number, { head: string; base: string }>();
  const lastGetCompute = new Map<number, number>();
  const pushQ: Job[] = [], getQ: Job[] = [];
  const queuedPush = new Set<number>(), queuedGet = new Set<number>();
  const dispatchLat: number[] = [], getStats = { total: 0, current: 0, nullOver: 0, nullCoalesced: 0, queued: 0 };
  const computes = { push: 0, get: 0, list: 0 }, conflicts = { n: 0 };
  let lists = 0, cpuUsed = 0, inflight = 0, busyCpu = 0;
  const cpuPerMerge: number[] = [], wallPerMerge: number[] = [];
  const perSecond: number[][] = [];
  let tokens = budget, tokensAt = Date.now();
  const t0 = Date.now(), end = t0 + seconds * 1000;

  // Head pushes: agent p pushes at phase p/PRS of every 2 minutes; each push is a `synchronize` dispatch.
  const pushes = (async () => {
    let n = 0;
    for (let i = 0; ; i++) {
      const p = i % PRS, at = t0 + (i * PUSH_EVERY_S * 1000) / PRS;
      if (at >= end) break;
      if (at > Date.now()) await sleep(at - Date.now());
      void commitOn(heads[p], prPaths(p, ++n), `push ${p} ${n}`).then((c) => {
        heads[p] = c;
        if (!queuedPush.has(p)) { queuedPush.add(p); pushQ.push({ pr: p, head: c, src: "push", at: Date.now(), dispatchAt: Date.now() }); }
        else { const j = pushQ.find((x) => x.pr === p)!; j.head = c; }
      });
    }
  })();
  // Base moves: 10 a minute, each changing 3 files anywhere.
  const moves = (async () => {
    for (let i = 1; ; i++) {
      const at = t0 + (i * 60_000) / BASE_MOVES_PER_MIN;
      if (at >= end) break;
      if (at > Date.now()) await sleep(at - Date.now());
      const paths = Array.from({ length: TOUCH }, () => { const f = Math.floor(Math.random() * files); return `pkg/${f % 50}/d${Math.floor(f / 50) % 20}/f${f}.txt`; });
      base = await commitOn(base, paths, `base ${i}`);
    }
  })();
  // Polls: each of 500 clients GETs its PR every 5 s (100/s); lists 10/s. A GET answers from `current` when it matches
  // the PR's head and the base tip; otherwise it may queue one compute (at most once per PR per 30 s) and answers null.
  const polls = (async () => {
    for (let i = 0; ; i++) {
      const at = t0 + (i * POLL_EVERY_S * 1000) / PRS;
      if (at >= end) break;
      if (at > Date.now()) await sleep(at - Date.now());
      const p = i % PRS;
      getStats.total++;
      const cur = current.get(p);
      if (cur && cur.head === heads[p] && cur.base === base) { getStats.current++; continue; }
      if (Date.now() - (lastGetCompute.get(p) ?? 0) < GET_COALESCE_MS || queuedPush.has(p)) { getStats.nullCoalesced++; continue; }
      if (!queuedGet.has(p)) { queuedGet.add(p); getQ.push({ pr: p, head: heads[p], src: "get", at: Date.now() }); getStats.queued++; lastGetCompute.set(p, Date.now()); }
      else getStats.nullCoalesced++;
      if (i % (PRS / (LISTS_PER_S * POLL_EVERY_S)) === 0) lists++; // list fields read `current` only, never compute
    }
  })();
  // The shard's test-merge worker: pushes and dispatch first, then GETs, within the CPU budget.
  const worker = (async () => {
    while (Date.now() < end + 5_000 || inflight) {
      const now = Date.now();
      const est = cpuPerMerge.length ? cpuPerMerge.slice(-50).reduce((a, b) => a + b, 0) / Math.min(50, cpuPerMerge.length) : 0.005;
      // The bucket holds one second of budget, or one merge if a merge costs more than that.
      tokens = Math.min(Math.max(budget, est), tokens + (budget * (now - tokensAt)) / 1000); tokensAt = now;
      const job = inflight < slots && tokens >= est ? (pushQ.shift() ?? (Date.now() < end ? getQ.shift() : undefined)) : undefined;
      if (!job) {
        // Over budget, a queued GET is dropped and answered null; it may queue again after 30 s.
        if (tokens < est && getQ.length > 50) { const d = getQ.shift()!; queuedGet.delete(d.pr); getStats.nullOver++; }
        await sleep(5); continue;
      }
      (job.src === "push" ? queuedPush : queuedGet).delete(job.pr);
      const head = heads[job.pr], b = base;
      if (job.src === "get" && current.get(job.pr)?.head === head && current.get(job.pr)?.base === b) continue;
      inflight++; tokens -= est;
      void mergeTree(b, head).then((r) => {
        inflight--; tokens -= r.cpu - est; cpuUsed += r.cpu; busyCpu += r.cpu;
        cpuPerMerge.push(r.cpu); wallPerMerge.push(r.wallMs); if (r.conflict) conflicts.n++;
        computes[job.src]++;
        current.set(job.pr, { head, base: b });
        if (job.dispatchAt) dispatchLat.push(Date.now() - job.dispatchAt);
      });
    }
  })();
  const sampler = (async () => { let last = 0; while (Date.now() < end) { await sleep(1_000); perSecond.push([Date.now() - t0, +(cpuUsed - last).toFixed(3), pushQ.length, getQ.length]); last = cpuUsed; } })();
  await Promise.all([pushes, moves, polls, sampler]);
  await worker;
  const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(s.length * p))] : null; };
  const wall = (Date.now() - t0) / 1000;
  const used = perSecond.map((x) => x[1]);
  const summary = {
    row: "B0-35", run: opt.run, at: new Date().toISOString(), seconds, k, prs: PRS, files, touch: TOUCH, budgetVcpu: budget,
    budgetOverride: !!opt.budget, slots,
    vcpuUsed: { mean: +(cpuUsed / wall).toFixed(3), p99PerSecond: pct(used, 0.99), maxPerSecond: pct(used, 1) },
    mergeCpuS: { n: cpuPerMerge.length, p50: pct(cpuPerMerge, 0.5), p99: pct(cpuPerMerge, 0.99), max: pct(cpuPerMerge, 1) },
    mergeWallMs: { p50: pct(wallPerMerge, 0.5), p99: pct(wallPerMerge, 0.99) },
    computes, conflicts: conflicts.n, lists, listComputes: computes.list,
    dispatchMs: { n: dispatchLat.length, p50: pct(dispatchLat, 0.5), p99: pct(dispatchLat, 0.99), max: pct(dispatchLat, 1) },
    gets: getStats, getCurrentShare: +(getStats.current / getStats.total).toFixed(3),
    eagerVcpuAtMeasuredCost: +(PRS * (BASE_MOVES_PER_MIN / 60) * (pct(cpuPerMerge, 0.5) ?? 0)).toFixed(2),
    eagerVcpuAtDesignCost: PRS * (BASE_MOVES_PER_MIN / 60) * 1.5,
    perSecond,
  };
  mkdirSync(`${ROOT}results/raw`, { recursive: true });
  writeFileSync(`${ROOT}results/raw/wp9-b0-35-${opt.run}.json`, JSON.stringify(summary, null, 1));
  console.log(JSON.stringify({ ...summary, perSecond: undefined }, null, 1));
}

const cmd = positionals[0];
if (cmd === "setup") setup();
else if (cmd === "run") await run();
else throw new Error("usage: node run/wp9-testmerge.ts setup | run");
