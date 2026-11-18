// BF-12 regression on `Push` commands through the real repository groups and the git edge (graft-m1-edge, test-faults
// build): "Leader killed after quorum and before the reply. The edge re-sends with the same RequestId. Pass: original
// outcomes returned, zero double applies."
//
//   node tests/m1/integ/bf12.ts --run NAME [--kills 1000] [--repos k0,...,k9] [--down-every 25]
//
// Each repository's ref `refs/heads/bf12/<run>` moves along a chain of object ids, one `Push` per kill, each expecting
// the previous id (`/_bed/push`, a single-ref push with no pack, under the writer's token). The first send carries the
// kill, alternating `after_quorum` and `after_quorum_unsynced`; every `down-every`-th kill holds the object down 12 s.
// `commit_hedged` re-sends under the same request id until it has an outcome. Per kill: the fault rows the members
// recorded (`landed`) and the original outcome in them, against the outcome the edge returned. At the end, every
// member's audit (request ids at two indexes, `dup` rows) and the ref, which must be the chain's last id: a push
// applied twice would be refused stale and change the outcome, and a push lost would leave the ref behind.
// Rows go to tests/m1/integ/raw/<run>.bf12.jsonl, so the run resumes.

import { createHash } from "node:crypto";
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: {
  run: { type: "string" }, kills: { type: "string", default: "1000" },
  repos: { type: "string", default: "k0,k1,k2,k3,k4,k5,k6,k7,k8,k9" }, "down-every": { type: "string", default: "25" },
} });
if (!opt.run) throw new Error("usage: bf12.ts --run NAME [--kills N] [--repos a,b]");
const ROOT = new URL("./", import.meta.url).pathname;
const RAW = `${ROOT}raw/`;
mkdirSync(RAW, { recursive: true });
const HOST = `https://graft-m1-edge.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const DIR = `${homedir()}/.graft-integ`;
const BED = readFileSync(`${homedir()}/.graft-wp6/bed.token`, "utf8").trim();
const WRITER = readFileSync(`${DIR}/tokens.txt`, "utf8").split("\n")[1].split(" ")[0];
const IDS: Record<string, number> = JSON.parse(readFileSync(`${DIR}/repos.json`, "utf8"));
const log = (m: string) => console.log(`${new Date().toISOString()} ${m}`);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const out = `${RAW}${opt.run}.bf12.jsonl`;
const rows = (): any[] => existsSync(out) ? readFileSync(out, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : [];

async function bed(path: string, repo: number, q = "", body?: unknown, auth = false): Promise<any> {
  for (let i = 0; ; i++) {
    try {
      const headers: Record<string, string> = { "x-bed-token": BED };
      if (auth) headers.authorization = `token ${WRITER}`;
      const r = await fetch(`${HOST}/_bed/${path}?repo=${repo}${q}`, {
        method: "POST", headers, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(180_000),
      });
      const t = await r.text();
      if (!r.ok) throw new Error(`${r.status} ${t.slice(0, 300)}`);
      return JSON.parse(t);
    } catch (e) {
      if (i >= 4) throw e;
      log(`${path} ${repo}: ${String(e).slice(0, 200)}; retrying`);
      await sleep(3_000 * (i + 1));
    }
  }
}

const oid = (repo: number, i: number) => i < 0 ? null : createHash("sha1").update(`bf12/${opt.run}/${repo}/${i}`).digest("hex");
const ref = `refs/heads/bf12/${opt.run}`;
/** Same outcome: the same index and the same per-ref result (the quorum a replay carries may differ). */
const key = (o: any) => o?.ok ? ["ok", o.ok.index, o.ok.out?.refs] : o?.refused ? ["refused", o.refused.index, o.refused.err] : o ? ["notLeader"] : null;
const same = (a: any, b: any) => a !== null && b !== null && JSON.stringify(key(a)) === JSON.stringify(key(b));

const names = opt.repos!.split(",");
const per = Math.ceil(Number(opt.kills) / names.length);
const downEvery = Number(opt["down-every"]);
const t0 = Date.now();
const done = rows();
await Promise.all(names.map(async (name) => {
  const repo = IDS[name];
  if (!repo) throw new Error(`no repository ${name} in ${DIR}/repos.json`);
  let i = done.filter((r) => r.repo === repo).length;
  while (i < per) {
    const mode = i % 2 === 0 ? "after_quorum" : "after_quorum_unsynced";
    const down = (i + 1) % downEvery === 0 ? 12_000 : 0;
    const s = Date.now();
    const r = await bed("push", repo, "", { ref, expect: oid(repo, i - 1), new: oid(repo, i), kill: mode, down_ms: down }, true)
      .catch((e) => ({ error: String(e) }));
    const ms = Date.now() - s;
    // The edge's outcome is a Rust `Result` in serde form: `{"Ok": <PushOutcome>}` or `{"Err": <CallErrorWire>}`.
    if (r.outcome && "Ok" in r.outcome) r.outcome = r.outcome.Ok;
    else if (r.outcome && "Err" in r.outcome) { r.error = JSON.stringify(r.outcome.Err); r.outcome = null; }
    if (!r.rid) { log(`${name} kill ${i}: ${JSON.stringify(r).slice(0, 300)}`); await sleep(5_000); continue; }
    const faults: any[] = await bed("faults", repo, `&rid=${r.rid}`);
    const landed = faults.flatMap((f) => (Array.isArray(f.r) ? f.r : []).map((x: any) => ({ site: f.site, ...x })));
    const original = landed[0]?.result ?? null;
    const row = { repo, name, i, rid: r.rid, mode, down_ms: down, ms, landed: landed.length, killedSite: landed[0]?.site ?? null,
      original, outcome: r.outcome, error: r.error, same: same(original, r.outcome),
      refOk: r.outcome?.ok?.out?.refs?.[0] === "Ok" };
    appendFileSync(out, JSON.stringify(row) + "\n");
    if (!row.same || row.landed !== 1 || !row.refOk) log(`${name} kill ${i}: ${JSON.stringify(row).slice(0, 500)}`);
    i++;
    if (i % 20 === 0) log(`${name}: ${i}/${per}, ${Math.round((Date.now() - t0) / 1000)} s`);
  }
}));
log("kills done; auditing");

const all = rows();
const per_repo = [];
for (const name of names) {
  const repo = IDS[name];
  const mine = all.filter((r) => r.repo === repo);
  // One more push without a kill, so every follower learns the last commit index.
  const last = Math.max(...mine.map((r) => r.i));
  const tail = await bed("push", repo, "", { ref: `${ref}-tail`, expect: null, new: oid(repo, 0) }, true);
  if (tail.outcome && "Ok" in tail.outcome) tail.outcome = tail.outcome.Ok;
  await sleep(3_000);
  const audit: any[] = await bed("audit", repo);
  const refs: [string, string][] = await bed("refs", repo);
  const at = refs.find(([n]) => n === ref)?.[1] ?? null;
  per_repo.push({
    name, repo, kills: mine.length, tail: !!tail.outcome?.ok, ref: at, refAtLast: at === oid(repo, last),
    members: audit.map((m) => ({ site: m.site, commit: m.r?.commit, applied: m.r?.applied, requestIds: m.r?.requestIds,
      requestRows: m.r?.requestRows, logTwice: m.r?.logTwice?.length ?? null, dupRows: m.r?.dupRows?.length ?? null, error: m.r?.error ?? null })),
  });
}
const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(p * s.length))] ?? null; };
const byMode = Object.fromEntries(["after_quorum", "after_quorum_unsynced"].flatMap((m) => [false, true].map((d) => {
  const xs = all.filter((r) => r.mode === m && (r.down_ms > 0) === d);
  return [`${m}${d ? "+down" : ""}`, { kills: xs.length, landed: xs.filter((r) => r.landed === 1).length, same: xs.filter((r) => r.same).length,
    answeredByOther: xs.filter((r) => r.outcome?.ok && r.killedSite !== null && !r.outcome.ok.quorum.includes(r.killedSite)).length,
    p50: pct(xs.map((r) => r.ms), 0.5), p99: pct(xs.map((r) => r.ms), 0.99) }];
})));
const report = {
  run: opt.run, at: new Date().toISOString(), kills: all.length, landed: all.filter((r) => r.landed === 1).length,
  landedTwice: all.filter((r) => r.landed > 1).length, same: all.filter((r) => r.same).length,
  changed: all.filter((r) => r.original && r.outcome && !r.same).length, noOutcome: all.filter((r) => !r.outcome).length,
  refOk: all.filter((r) => r.refOk).length,
  doubleApplies: per_repo.reduce((n, g) => n + g.members.reduce((k: number, m: any) => k + (m.logTwice ?? 0) + (m.dupRows ?? 0), 0), 0),
  refsAtLast: per_repo.filter((g) => g.refAtLast).length, byMode, repos: per_repo,
};
writeFileSync(`${RAW}${opt.run}.bf12-report.json`, JSON.stringify(report, null, 2));
const { repos: _, ...summary } = report;
log(JSON.stringify(summary));
