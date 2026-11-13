// M1-WP2 gate runner on the deployed test install (graft-m1-site-a/b/c, edge graft-m1-wp2-edge). Writes raw rows to
// tests/m1/wp2/raw/ and resumes from them, so it can run as a supervised service that restarts.
//
//   node tests/m1/wp2/run.ts bf12  --run NAME [--kills 10000] [--groups 10] [--down-every 25]
//   node tests/m1/wp2/run.ts bf13  --run NAME [--rounds 400] [--groups 8] [--early]
//   node tests/m1/wp2/run.ts bench --run NAME [--seconds 60] [--rate 100] [--bytes 1024] [--near-ms MS]
//                                  [--chain repo:N] [--body array|stream] [--arrival sql|mem] [--entry soft|max]   # reuse a group; B0-02's A/B switches
//   node tests/m1/wp2/run.ts audit --run NAME      # BF-12 audit of every group's three members
//
// BF-12: each group appends unique tokens `i,` to one register, one command per kill. Kill modes alternate
// `after_quorum` (the commit-advance writes are durable, the reply is dropped, the object resets) and
// `after_quorum_unsynced` (the commit-advance writes are dropped too); every `down-every`-th kill also holds the reset
// object down for 12 s, so another member must lead and answer the re-send. BF-13 rounds are described in edge.ts.

import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  run: { type: "string" }, kills: { type: "string", default: "10000" }, groups: { type: "string" },
  "down-every": { type: "string", default: "25" }, rounds: { type: "string", default: "400" },
  seconds: { type: "string", default: "60" }, rate: { type: "string", default: "100" }, bytes: { type: "string", default: "1024" },
  early: { type: "boolean", default: false }, "near-ms": { type: "string" }, body: { type: "string" }, chain: { type: "string" }, arrival: { type: "string" }, entry: { type: "string" },
} });
const row = positionals[0];
if (!opt.run || !row) throw new Error("usage: node tests/m1/wp2/run.ts bf12|bf13|bench|audit --run NAME");
const ROOT = new URL("./", import.meta.url).pathname;
const RAW = `${ROOT}raw/`;
mkdirSync(RAW, { recursive: true });
const URL_ = process.env.EDGE_URL ?? "https://graft-m1-wp2-edge.<workers-subdomain>.workers.dev";
const TOKEN = readFileSync(`${ROOT}../../m0/.m0-token`, "utf8").trim();
const log = (m: string) => console.log(`${new Date().toISOString()} ${m}`);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function edge<T = any>(path: string, body?: unknown, ms = 600_000): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      const res = await fetch(URL_ + path, {
        method: body === undefined ? "GET" : "POST", body: body === undefined ? undefined : JSON.stringify(body),
        headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(ms),
      });
      const text = await res.text();
      if (!res.ok) throw new Error(`${res.status} ${text.slice(0, 300)}`);
      return JSON.parse(text) as T;
    } catch (e) {
      if (attempt >= 4) throw e;
      log(`${path} failed (${String(e).slice(0, 160)}); retrying`);
      await sleep(2_000 * (attempt + 1));
    }
  }
}

const readRows = (path: string): any[] => existsSync(path)
  ? readFileSync(path, "utf8").split("\n").filter(Boolean).map((l) => JSON.parse(l)) : [];
const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(s.length * p))]; };

/** The run's groups, created once and recorded so a restart reuses them. */
async function groups(n: number, base: number): Promise<string[]> {
  const path = `${RAW}${opt.run}.groups.json`;
  if (existsSync(path)) return JSON.parse(readFileSync(path, "utf8"));
  const out: string[] = [];
  for (let g = 0; g < n; g++) {
    const chain = `repo:${base + g}`;
    const r = await edge("/create", { chain, ...(opt["near-ms"] ? { nearMs: Number(opt["near-ms"]) } : {}) });
    if (!r.warm?.ok) throw new Error(`create ${chain}: ${JSON.stringify(r).slice(0, 400)}`);
    log(`created ${chain}: ${r.verdict} in ${r.leaderMs} ms; placed ${JSON.stringify(r.placed)}`);
    writeFileSync(`${RAW}${opt.run}.create.json`, JSON.stringify(r.placed ?? null));
    out.push(chain);
  }
  writeFileSync(path, JSON.stringify(out));
  return out;
}

function base(): number {
  // A run's chains are its own, so reruns never share members: 1000 chains per run name hash.
  let h = 0;
  for (const c of opt.run!) h = (h * 31 + c.charCodeAt(0)) % 9_000;
  return 10_000 + h * 1_000;
}

async function bf12(): Promise<void> {
  const total = Number(opt.kills), ng = Number(opt.groups ?? 10), downEvery = Number(opt["down-every"]);
  const chains = await groups(ng, base());
  const out = `${RAW}${opt.run}.bf12.jsonl`;
  const done = readRows(out);
  const per = Math.ceil(total / ng);
  const t0 = Date.now();
  await Promise.all(chains.map(async (chain, g) => {
    let have = done.filter((r) => r.chain === chain).length;
    while (have < per) {
      const n = Math.min(20, per - have);
      const plan = Array.from({ length: n }, (_, j) => {
        const i = have + j;
        const mode = i % 2 === 0 ? "after_quorum" : "after_quorum_unsynced";
        return (i + 1) % downEvery === 0 ? { mode, down_ms: 12_000 } : { mode };
      });
      const r = await edge<{ colo: string; kills: any[] }>("/bf12", { chain, key: "bf12:log", from: have, plan }).catch((e) => ({ colo: "", kills: [], error: String(e) }));
      if (!r.kills.length) { log(`${chain}: batch failed ${(r as any).error ?? ""}`); await sleep(5_000); continue; }
      for (const k of r.kills) appendFileSync(out, JSON.stringify({ chain, i: Number(k.token.slice(0, -1)), colo: r.colo, ...k }) + "\n");
      have += r.kills.length;
      const bad = r.kills.filter((k) => !k.same).length;
      if (g === 0 || bad) log(`${chain}: ${have}/${per} kills, ${bad} not same in this batch, ${Math.round((Date.now() - t0) / 1000)} s`);
    }
  }));
  log("bf12 kills done");
  await audit();
}

/** BF-12's audit: every member of every group, its committed log and its register. */
async function audit(): Promise<void> {
  const chains: string[] = JSON.parse(readFileSync(`${RAW}${opt.run}.groups.json`, "utf8"));
  const rows = readRows(`${RAW}${opt.run}.bf12.jsonl`);
  // Let every follower learn the last commit index: one more write per group.
  for (const chain of chains) await edge("/op", { chain, op: { kind: "put", key: "sync", value: String(Date.now()) } });
  await sleep(3_000);
  const groupsOut = [];
  for (const chain of chains) {
    const mine = rows.filter((r) => r.chain === chain);
    const members = await edge<{ site: number; r: any }[]>(`/audit?chain=${chain}&prefix=bf12:`, undefined, 120_000);
    const perMember = members.map(({ site, r }) => {
      const v: string = r?.values?.["bf12:log"] ?? "";
      const tokens = v.split(",").filter(Boolean).map(Number);
      const counts = new Map<number, number>();
      for (const t of tokens) counts.set(t, (counts.get(t) ?? 0) + 1);
      return {
        site, commit: r?.commit, applied: r?.applied, requestIds: r?.requestIds, requestRows: r?.requestRows,
        logTwice: r?.logTwice?.length ?? null, dupRows: r?.dupRows?.length ?? null,
        tokens: tokens.length, distinct: counts.size, twice: [...counts].filter(([, c]) => c > 1).map(([t]) => t),
        missing: mine.filter((k) => k.outcome?.ok && !counts.has(k.i)).map((k) => k.i),
        diskSame: r?.diskValues?.["bf12:log"] === r?.values?.["bf12:log"],
        inOrder: tokens.every((t, j) => j === 0 || t > tokens[j - 1]),
      };
    });
    groupsOut.push({ chain, kills: mine.length, members: perMember });
  }
  const s = {
    run: opt.run, at: new Date().toISOString(), groups: chains.length,
    kills: rows.length,
    landed: rows.filter((r) => r.landed === 1).length,
    landedTwice: rows.filter((r) => r.landed > 1).length,
    notLanded: rows.filter((r) => r.landed === 0).length,
    sameOutcome: rows.filter((r) => r.same).length,
    changedOutcome: rows.filter((r) => r.landed && r.outcome && !r.same).map((r) => ({ chain: r.chain, i: r.i, original: r.original, outcome: r.outcome })),
    noOutcome: rows.filter((r) => !r.outcome).map((r) => ({ chain: r.chain, i: r.i, error: r.error })),
    byMode: Object.fromEntries(["after_quorum", "after_quorum_unsynced"].flatMap((m) => [0, 12_000].map((d) => {
      const xs = rows.filter((r) => r.mode === m && r.down_ms === d);
      return [`${m}${d ? "+down" : ""}`, { kills: xs.length, landed: xs.filter((r) => r.landed === 1).length, same: xs.filter((r) => r.same).length,
        answeredByOther: xs.filter((r) => r.outcome?.ok && r.killedSite !== null && !(r.outcome.ok.quorum as number[]).includes(r.killedSite)).length,
        resendMs: { p50: pct(xs.map((r) => r.ms), 0.5), p99: pct(xs.map((r) => r.ms), 0.99), max: pct(xs.map((r) => r.ms), 1) },
        sends: { p50: pct(xs.map((r) => r.sends), 0.5), max: pct(xs.map((r) => r.sends), 1) } }];
    }))),
    doubleApplies: groupsOut.reduce((n, g) => n + g.members.reduce((m, x) => m + x.twice.length + (x.logTwice ?? 0) + (x.dupRows ?? 0), 0), 0),
    groupsDetail: groupsOut,
  };
  writeFileSync(`${RAW}${opt.run}.bf12-audit.json`, JSON.stringify(s, null, 2));
  log(JSON.stringify({ ...s, groupsDetail: undefined, changedOutcome: s.changedOutcome.length, noOutcome: s.noOutcome.length }));
}

async function bf13(): Promise<void> {
  const total = Number(opt.rounds), ng = Number(opt.groups ?? 8);
  const chains = await groups(ng, base());
  const out = `${RAW}${opt.run}.bf13.jsonl`;
  const done = readRows(out);
  const per = Math.ceil(total / ng);
  // `before_commit_early`: the leader's AppendEntries leave before its own write, and it resets before the write.
  const modes = opt.early ? [
    { mode: "before_commit" }, { mode: "before_commit_sent" }, { mode: "before_commit_early" },
    { mode: "before_commit", down_ms: 12_000 }, { mode: "before_commit_sent", down_ms: 12_000 },
    { mode: "before_commit_early", down_ms: 12_000 },
  ] : [
    { mode: "before_commit" }, { mode: "before_commit_sent" },
    { mode: "before_commit", down_ms: 12_000 }, { mode: "before_commit_sent", down_ms: 12_000 },
  ];
  await Promise.all(chains.map(async (chain, g) => {
    let have = done.filter((r) => r.chain === chain).length;
    while (have < per) {
      const m = modes[have % modes.length];
      const key = `bf13:${have}`;
      const client0 = (g * per + have) * 4;
      const r = await edge("/bf13", { chain, key, client0, ...m }).catch((e) => ({ error: String(e) }));
      if ((r as any).error) { log(`${chain} ${key}: ${(r as any).error.slice(0, 200)}`); await sleep(5_000); continue; }
      appendFileSync(out, JSON.stringify({ chain, round: have, ...m, ...r }) + "\n");
      have++;
      if (g === 0 && have % 10 === 0) log(`${chain}: ${have}/${per} rounds`);
    }
  }));
  log("bf13 rounds done");
  await bf13Report();
}

async function bf13Report(): Promise<void> {
  const rows = readRows(`${RAW}${opt.run}.bf13.jsonl`);
  const chains = [...new Set(rows.map((r) => r.chain))];
  // Every member's log, for the refusal audit: a refusal returned at index i is from a committed entry only if i is
  // committed and, when the lost entry also sat at i, the committed entry there is not the lost one.
  const logs = new Map<string, { site: number; rows: any[] }[]>();
  for (const chain of chains) {
    const all: { site: number; rows: any[] }[] = [];
    for (let from = 1; ; from += 500) {
      const part = await edge<{ site: number; r: any[] | null }[]>(`/log?chain=${chain}&from=${from}&n=500`, undefined, 120_000);
      for (const p of part) {
        let m = all.find((x) => x.site === p.site);
        if (!m) all.push(m = { site: p.site, rows: [] });
        m.rows.push(...(p.r ?? []));
      }
      if (part.every((p) => (p.r ?? []).length < 500)) break;
    }
    logs.set(chain, all);
  }
  const committedAt = (chain: string, index: number) => {
    for (const m of logs.get(chain) ?? []) {
      const e = m.rows.find((x) => x.index === index);
      if (e?.committed) return e.digest as string;
    }
    return null;
  };
  // Each group holds its own registers, so the checker's key is the chain and the key.
  const history = rows.flatMap((r) => r.history.map((h: any) => ({ ...h, op: { ...h.op, key: `${r.chain}/${h.op.key}` } })));
  const hpath = `${RAW}${opt.run}.bf13-history.jsonl`;
  writeFileSync(hpath, history.map((h) => JSON.stringify(h)).join("\n") + "\n");
  const refusals = rows.filter((r) => r.results?.cas2?.refused);
  // A refusal names the entry it rests on; it came from an uncommitted entry if no member holds that index committed.
  const fromUncommitted = refusals.filter((r) => committedAt(r.chain, r.results.cas2.refused.index) === null);
  const lostCommitted = rows.filter((r) => r.fault && committedAt(r.chain, r.fault.index) === r.fault.result.digest);
  const s = {
    run: opt.run, at: new Date().toISOString(), rounds: rows.length,
    landed: rows.filter((r) => r.landed === 1).length,
    staleInEntry: rows.filter((r) => r.fault?.result?.refusedInEntry?.includes(r.ids.cas2)).length,
    byMode: Object.fromEntries([...new Set(rows.map((r) => `${r.mode}${r.down_ms ? "+down" : ""}`))].map((m) => {
      const xs = rows.filter((r) => `${r.mode}${r.down_ms ? "+down" : ""}` === m);
      return [m, {
        rounds: xs.length,
        staleInEntry: xs.filter((r) => r.fault?.result?.refusedInEntry?.includes(r.ids.cas2)).length,
        killedEntryCommittedLater: xs.filter((r) => r.fault && committedAt(r.chain, r.fault.index) === r.fault.result.digest).length,
        cas2Ok: xs.filter((r) => r.results?.cas2?.ok).length,
        cas2Refused: xs.filter((r) => r.results?.cas2?.refused).length,
        cas2NoAnswer: xs.filter((r) => !r.results?.cas2).length,
        readA: xs.filter((r) => r.read?.value === "a").length,
        readB: xs.filter((r) => r.read?.value === "b").length,
      }];
    })),
    killedEntriesCommittedLater: lostCommitted.length,
    refusalsReturned: refusals.length,
    refusalsFromUncommittedEntries: fromUncommitted.map((r) => ({ chain: r.chain, round: r.round, refused: r.results.cas2.refused, fault: r.fault })),
    historyOps: history.length,
    historyFile: hpath,
  };
  writeFileSync(`${RAW}${opt.run}.bf13-report.json`, JSON.stringify(s, null, 2));
  log(JSON.stringify(s));
}

async function bench(): Promise<void> {
  const chain = opt.chain ?? (await groups(1, base()))[0];
  await edge("/stats", { chain, reset: true, body: opt.body, arrival: opt.arrival, entry: opt.entry ?? "soft" });
  const seconds = Number(opt.seconds), rate = Number(opt.rate);
  // As M0's raft-bench.ts: 20 callers x 5 workers x 1 op/s = 100 commands/s, each call about 15 s.
  const WORKERS = 5, CHUNK_S = 15, callers = Math.ceil(rate / WORKERS);
  const perWorker = rate / callers / WORKERS;
  const client: number[] = []; let failed = 0, done = 0, colo = "";
  const t0 = Date.now();
  while (Date.now() - t0 < seconds * 1000) {
    const rs = await Promise.all(Array.from({ length: callers }, () =>
      edge<any>("/bench", { chain, n: Math.round(WORKERS * perWorker * CHUNK_S), concurrency: WORKERS, bytes: Number(opt.bytes), ratePerWorker: perWorker }).catch((e) => ({ error: String(e) }))));
    for (const r of rs) {
      if (r.error) { log(r.error); continue; }
      client.push(...r.lat); failed += r.failed; done += r.n - r.failed; colo = r.colo;
    }
  }
  const wallS = (Date.now() - t0) / 1000;
  const stats = await edge<{ site: number; r: any }[]>("/stats", { chain });
  if (process.env.BENCH_RAW) writeFileSync(`${RAW}${opt.run}.stats.json`, JSON.stringify(stats));
  const status = await edge<{ site: number; r: any }[]>(`/status?chain=${chain}`);
  const leader = stats.find((s) => s.r?.role === true)!;
  // The leader's measured round trips to each follower (`Rtt`): B0-02's commit tail follows the near one.
  const rtt = await edge<{ body: string }>(`/raw?chain=${chain}&site=${leader.site}&path=/rtt`)
    .then((r) => Object.fromEntries(JSON.parse(r.body).map(([, x]: any) => [x.peer, x.median_ms])))
    .catch(() => null);
  const lat: number[] = leader.r.commitMs, round: number[] = leader.r.roundMs;
  const s = {
    row: "b0-01 (M1-WP2 Rust host)", run: opt.run, body: opt.body ?? "stream", arrival: opt.arrival ?? "sql", entry: opt.entry ?? "soft", bytes: Number(opt.bytes), chain, at: new Date().toISOString(), seconds: wallS, edgeColo: colo, leader: leader.site, rtt,
    commands: done, failed, throughputPerS: Math.round(done / wallS), entries: leader.r.entries,
    commandsPerEntry: Math.round((leader.r.commands / leader.r.entries) * 10) / 10,
    commitMs: { p50: pct(lat, 0.5), p99: pct(lat, 0.99), max: pct(lat, 1), n: lat.length },
    roundMs: { p50: pct(round, 0.5), p99: pct(round, 0.99), n: round.length },
    // The leader's own storage confirm, from its write to `sync()`; its AppendEntries no longer wait for it.
    leaderConfirmMs: { p50: pct(leader.r.confirmMs, 0.5), p90: pct(leader.r.confirmMs, 0.9), p99: pct(leader.r.confirmMs, 0.99), n: leader.r.confirmMs.length },
    appendMs: Object.fromEntries(Object.entries(leader.r.appendMs as Record<string, number[]>).map(([k, v]) => [k, { p50: pct(v, 0.5), p99: pct(v, 0.99), n: v.length }])),
    clientMs: { p50: pct(client, 0.5), p99: pct(client, 0.99) },
    failedCalls: leader.r.failedCalls, alarms: leader.r.alarms,
    verdict: pct(lat, 0.99) <= 600 ? "Pass" : pct(lat, 0.99) > 1000 ? "Fail" : "Between pass and fail",
    status: status.map((x) => ({ site: x.site, role: x.r?.role, term: x.r?.term, last: x.r?.last, commit: x.r?.commit, restores: x.r?.restores })),
  };
  writeFileSync(`${RAW}${opt.run}.bench.json`, JSON.stringify(s, null, 2));
  log(JSON.stringify(s));
}

if (row === "bf12") await bf12();
else if (row === "bf13") await bf13();
else if (row === "bench") await bench();
else if (row === "audit") await audit();
else if (row === "bf13-report") await bf13Report();
// A supervised service restarts its command on exit; a finished run idles instead.
if (process.env.STAY) await new Promise(() => {});
