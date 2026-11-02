// BF-19 on the hosted cron with the real `request` table: "1,000 failed placements plus 1,000 commits whose marker
// write is killed. Pass: failed ones deleted after 24 h, zero committed ones deleted, missing markers repaired."
//
//   node tests/m1/wp8/bf19.ts --run NAME --repo ID [--n 1000] [--par 10] [--stage seed|sweep|all]
//
// Seed, through graft-m1-edge's commit path on the repository's real Raft group (`/_bed/push`):
// - 1,000 failed placements: a stored pack of 1 MiB + 1 byte placed by `PackStore::place_teed` with every bucket but
//   the record home's refusing it (`fail_place`), so the two-region rule fails and the edge never commits; one copy
//   stays in the home bucket, named in its custom metadata by the chain and a request id that has no `request` row.
// - 1,000 commits whose marker write is killed: a stored pack placed in the buckets, then a `Push` naming it,
//   committed by the real leader, which skips its `Committed` marker puts (`no_marker`), as a leader that dies after
//   `Acked` and before them. The `request` row is the real one `GroupFold::apply` wrote.
// Then the site's own sweeper (`sweep::run_at`, the cron handler's body) on the network's home site, through the
// edge's `/_bed/sweep?site=N&now=MS`, with its clock at: real time (marker repairs), +23 h (nothing deleted), the
// first failed placement + 24 h 1 min, the last + 24 h 1 min, and the last + 72 h. Its clock is the only fake.
// Counts come from the buckets: a HEAD of each seeded pack and its marker in every bucket (`/_bed/heads`).
// Rows go to tests/m1/wp8/raw/<run>.bf19.jsonl, so a seed resumes.

import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { homedir } from "node:os";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: {
  run: { type: "string" }, repo: { type: "string" }, n: { type: "string", default: "1000" },
  par: { type: "string", default: "10" }, stage: { type: "string", default: "all" }, home: { type: "string", default: "0" },
} });
if (!opt.run || !opt.repo) throw new Error("usage: bf19.ts --run NAME --repo ID");
const ROOT = new URL("./", import.meta.url).pathname;
const RAW = `${ROOT}raw/`;
mkdirSync(RAW, { recursive: true });
const HOST = "https://graft-m1-edge.<workers-subdomain>.workers.dev";
const DIR = `${homedir()}/.graft-integ`;
const BED = readFileSync(`${homedir()}/.graft-wp6/bed.token`, "utf8").trim();
const WRITER = readFileSync(`${DIR}/tokens.txt`, "utf8").split("\n")[1].split(" ")[0];
const REPO = Number(opt.repo), N = Number(opt.n), PAR = Number(opt.par), HOME = Number(opt.home);
const SIZE = (1 << 20) + 1;
const log = (m: string) => console.log(`${new Date().toISOString()} ${m}`);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const out = `${RAW}${opt.run}.bf19.jsonl`;
const rows = (): any[] => existsSync(out) ? readFileSync(out, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : [];

async function bed(path: string, q: string, body?: unknown, auth = false, tries = 5): Promise<any> {
  for (let i = 0; ; i++) {
    try {
      const headers: Record<string, string> = { "x-bed-token": BED };
      if (auth) headers.authorization = `token ${WRITER}`;
      const r = await fetch(`${HOST}/_bed/${path}?${q}`, {
        method: "POST", headers, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(900_000),
      });
      const t = await r.text();
      if (!r.ok) throw new Error(`${r.status} ${t.slice(0, 300)}`);
      return JSON.parse(t);
    } catch (e) {
      if (i >= tries - 1) throw e;
      log(`${path} ${q.slice(0, 60)}: ${String(e).slice(0, 200)}; retrying`);
      await sleep(3_000 * (i + 1));
    }
  }
}

const oid = (k: string, i: number) => createHash("sha1").update(`bf19/${opt.run}/${k}/${i}`).digest("hex");

async function seed() {
  const have = rows();
  const failed = new Set(have.filter((r) => r.kind === "failed").map((r) => r.i));
  const committed = new Set(have.filter((r) => r.kind === "committed").map((r) => r.i));
  const todo: [string, number][] = [];
  for (let i = 0; i < N; i++) {
    if (!failed.has(i)) todo.push(["failed", i]);
    if (!committed.has(i)) todo.push(["committed", i]);
  }
  log(`seeding ${todo.length} of ${2 * N}`);
  let n = 0;
  await Promise.all([...Array(PAR).keys()].map(async () => {
    while (todo.length) {
      const [kind, i] = todo.shift()!;
      // Each committed push creates its own ref, so pushes do not depend on each other.
      const body = kind === "failed"
        ? { ref: `refs/heads/bf19/${opt.run}/f${i}`, expect: null, new: oid("f", i), stored: SIZE, fail_place: true }
        : { ref: `refs/heads/bf19/${opt.run}/c${i}`, expect: null, new: oid("c", i), stored: SIZE, no_marker: true };
      const r = await bed("push", `repo=${REPO}`, body, true).catch((e) => ({ error: String(e) }));
      if (r.outcome && "Ok" in r.outcome) r.outcome = r.outcome.Ok;
      const row = kind === "failed"
        ? { kind, i, at: Date.now(), rid: r.rid, id: r.placed?.id ?? null, placeError: r.placed?.error ?? null, error: r.error ?? null }
        : { kind, i, at: Date.now(), rid: r.rid, id: r.pack?.stored?.id ?? null, index: r.outcome?.ok?.index ?? null,
            ok: r.outcome?.ok?.out?.refs?.[0] === "Ok", sites: r.pack?.stored?.sites ?? null, error: r.error ?? null };
      if ((kind === "failed" && !row.id) || (kind === "committed" && !(row as any).ok)) {
        log(`${kind} ${i}: ${JSON.stringify(r).slice(0, 400)}; again later`);
        todo.push([kind, i]);
        await sleep(2_000);
        continue;
      }
      appendFileSync(out, JSON.stringify(row) + "\n");
      if (++n % 100 === 0) log(`seeded ${n}`);
    }
  }));
}

// The buckets' view of every seeded pack: does any bucket hold it, and does any hold its marker.
async function census(label: string) {
  const all = rows();
  const ids = all.map((r) => r.id);
  // 100 ids a call, 5 calls at once: one call's HEADs share its invocation's six connections.
  const heads: any[] = [];
  const chunks: string[][] = [];
  for (let k = 0; k < ids.length; k += 100) chunks.push(ids.slice(k, k + 100));
  for (let k = 0; k < chunks.length; k += 5) {
    for (const h of await Promise.all(chunks.slice(k, k + 5).map((c) => bed("heads", `repo=${REPO}`, { ids: c })))) heads.push(...h);
  }
  const by = new Map<string, any>(heads.map((h: any) => [h.id, h]));
  const count = (kind: string, f: (h: any) => boolean) => all.filter((r) => r.kind === kind && f(by.get(r.id))).length;
  const c = {
    label,
    failed: { held: count("failed", (h) => h.buckets.some((b: any) => b.pack)), marked: count("failed", (h) => h.buckets.some((b: any) => b.marker)) },
    committed: {
      held: count("committed", (h) => h.buckets.some((b: any) => b.pack)),
      inAllPlacedBuckets: count("committed", (h) => h.buckets.filter((b: any) => b.pack).length >= 2),
      markedEveryHolder: count("committed", (h) => h.buckets.every((b: any) => !b.pack || b.marker)),
      unmarked: count("committed", (h) => h.buckets.every((b: any) => !b.marker)),
      deleted: count("committed", (h) => h.buckets.every((b: any) => !b.pack)),
    },
  };
  log(`census ${label}: ${JSON.stringify(c)}`);
  return c;
}

async function sweep(label: string, now: number) {
  // The sweeper's budget (CronBudget::SITE_CRON, 600 actions) cuts a run; run until a run plans nothing more for the
  // network, as consecutive cron runs would. The stored cursor can start a run past this network, so a run that did
  // not reach it goes on until the cursor wraps twice.
  const runs: any[] = [];
  let wraps = 0;
  for (let k = 0; k < 20; k++) {
    const r = await bed("sweep", `site=${HOME}&now=${now}`, undefined, false, 3);
    const mine = (r.swept ?? []).find((s: any) => s.network === REPO);
    runs.push({ now, next: r.next, mine, failed: r.failed, unresolved: r.unresolved });
    log(`sweep ${label} run ${k}: ${JSON.stringify({ next: r.next, mine, failed: r.failed?.length, unresolved: r.unresolved?.length })}`);
    if (mine && mine.planned <= mine.done) break;
    if (!mine && r.next === null && ++wraps >= 2) break;
  }
  return { label, now, runs };
}

if (opt.stage === "seed" || opt.stage === "all") await seed();
if (opt.stage === "sweep" || opt.stage === "all") {
  const all = rows();
  const first = Math.min(...all.filter((r) => r.kind === "failed").map((r) => r.at));
  const last = Math.max(...all.map((r) => r.at));
  const H = 3_600_000, M = 60_000;
  const report: any = { run: opt.run, repo: REPO, n: N, seeded: { failed: all.filter((r) => r.kind === "failed").length,
    committed: all.filter((r) => r.kind === "committed").length, committedAt: [...new Set(all.filter((r) => r.kind === "committed").map((r) => r.index))].length },
    first, last, steps: [] };
  report.steps.push({ census: await census("before") });
  for (const [label, now] of [["real time", Date.now()], ["last + 23 h", last + 23 * H], ["first + 24 h 1 min", first + 24 * H + M],
    ["last + 24 h 1 min", last + 24 * H + M], ["last + 72 h", last + 72 * H]] as [string, number][]) {
    const s = await sweep(label, now);
    report.steps.push({ ...s, census: await census(label) });
  }
  writeFileSync(`${RAW}${opt.run}.bf19-report.json`, JSON.stringify(report, null, 2));
  log(JSON.stringify(report.steps.map((s: any) => s.census)));
}
