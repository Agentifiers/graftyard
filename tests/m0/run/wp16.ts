// M0-WP16 (L0-05, L0-06): drive the predictor shard `graft-wp16-shard` (src/wp16/shard.ts, wp16/shard/main.go).
//
//   node run/wp16.ts upload <dir>     multipart-upload <dir>/pack.pack, refs, shallow to graft-wp16-net under net/linux/
//   node run/wp16.ts load             start shard s0's container and load the network from R2
//   node run/wp16.ts load-all|stop-all [--shards 7]   the same, or stop, for shards s0..s(k-1)
//   node run/wp16.ts replay <jobs.json> [--rate 4.17] [--cap 8] [--order size|recent] [--leads-at 0.8] [--shards 1] [--pad] [--out name]
//
// `replay` sends each push of wp16/jobs.py's replay as one InsightJob at its accelerated push time (the consumer runs at
// once), `rate` pushes a second, to one shard whose container allows PREDICT_CPU * 4 vCPU git processes at once. It
// records push-to-verdict per job, the container's cgroup CPU every second, and each verdict against the offline
// ground truth. At `leads-at` (fraction of the jobs) it marks the shard's site as leading for 60 s of pushes and counts
// merge-trees the container runs meanwhile, which must be zero.
// Deploy: wrangler deploy -c wp16/graft-wp16-shard.jsonc, then `wrangler secret put M0_TOKEN`.

import { openSync, readFileSync, readSync, statSync, writeFileSync, mkdirSync } from "node:fs";

const ROOT = new URL("../", import.meta.url).pathname;
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const URL_ = "https://graft-wp16-shard.<workers-subdomain>.workers.dev";
const PREDICT_PAIRS = 8, PREDICT_CPU = 0.5, VCPU = 4;
const auth = { authorization: `Bearer ${TOKEN}` };

async function call<T = any>(path: string, body: unknown, timeoutMs = 300_000): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    const res = await fetch(URL_ + path, { method: "POST", body: JSON.stringify(body), headers: auth, signal: AbortSignal.timeout(timeoutMs) })
      .catch((e) => e as Error);
    if (res instanceof Error) { if (attempt < 2) continue; throw res; }
    const text = await res.text();
    if (res.ok) return JSON.parse(text) as T;
    if (attempt < 2 && res.status >= 500) continue;
    throw new Error(`${path}: ${res.status} ${text.slice(0, 400)}`);
  }
}

async function upload(dir: string): Promise<void> {
  for (const f of ["refs", "shallow"]) {
    const r = await fetch(`${URL_}/put?key=net/linux/${f}`, { method: "POST", headers: auth, body: readFileSync(`${dir}/${f}`) });
    console.log(f, r.status, await r.text());
  }
  const key = "net/linux/pack.pack", size = statSync(`${dir}/pack.pack`).size, PART = 25 << 20;
  const { uploadId } = await call<{ uploadId: string }>(`/mpu/start?key=${key}`, {});
  const fd = openSync(`${dir}/pack.pack`, "r"), n = Math.ceil(size / PART), parts: object[] = new Array(n);
  const t0 = Date.now();
  let next = 0;
  await Promise.all([0, 1].map(async () => {
    for (let i = next++; i < n; i = next++) {
      const buf = Buffer.alloc(Math.min(PART, size - i * PART));
      readSync(fd, buf, 0, buf.length, i * PART);
      for (let attempt = 0; ; attempt++) {
        // Node's fetch on this orb sometimes resets a large upload stream (HTTP/2 ENHANCE_YOUR_CALM); retry the part.
        const r = await fetch(`${URL_}/mpu/part?key=${key}&uploadId=${encodeURIComponent(uploadId)}&n=${i + 1}`, { method: "POST", headers: auth, body: buf })
          .catch((e) => e as Error);
        if (!(r instanceof Error) && r.ok) { parts[i] = await r.json(); break; }
        if (attempt > 5) throw new Error(`part ${i + 1}: ${r instanceof Error ? r.message : `${r.status} ${await r.text()}`}`);
        await sleep(2000);
      }
      console.log(`part ${i + 1}/${n}`);
    }
  }));
  const done = await fetch(`${URL_}/mpu/done?key=${key}&uploadId=${encodeURIComponent(uploadId)}`, { method: "POST", headers: auth, body: JSON.stringify(parts) });
  console.log(await done.text(), `${((Date.now() - t0) / 1000).toFixed(0)} s`);
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(p * s.length))] ?? 0; };
const arg = (name: string, d: string) => { const i = process.argv.indexOf(`--${name}`); return i > 0 ? process.argv[i + 1] : d; };

interface Other { ref: string; head: string; n: number; t: number; truth: string; landedTruth?: string }
interface Job { i: number; t: number; ref: string; head: string; base: string; live: number; others: Other[] }

async function replay(file: string): Promise<void> {
  const rate = Number(arg("rate", "4.17")), cap = Number(arg("cap", String(PREDICT_PAIRS))), order = arg("order", "size");
  const leadsAt = Number(arg("leads-at", "0.8")), out = arg("out", `wp16-replay-${Date.now()}`), limit = Number(arg("limit", "0"));
  // The second warm site's k shards (k = 7 for A1); each job goes to one shard by a hash of its ref, as rendezvous would.
  const shards = Array.from({ length: Number(arg("shards", "1")) }, (_, x) => `s${x}`);
  const shardOf = (ref: string) => shards[[...ref].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7) % shards.length];
  const cpuAll = async () => {
    const rs = await Promise.all(shards.map((shard) => call("/cpu", { shard })));
    return { usec: rs.reduce((a, r) => a + r.usec, 0), runs: rs.reduce((a, r) => a + r.runs, 0), hits: rs.reduce((a, r) => a + r.hits, 0),
      errs: rs.reduce((a, r) => a + r.errs, 0), per: rs.map((r) => r.usec) };
  };
  const setLeads = (leads: boolean) => Promise.all(shards.map((shard) => call("/leads", { shard, leads })));
  let jobs: Job[] = JSON.parse(readFileSync(file, "utf8")).jobs;
  if (limit) jobs = jobs.slice(0, limit);
  const leadIdx = leadsAt > 0 ? Math.floor(jobs.length * leadsAt) : -1, leadJobs = Math.round(60 * rate);
  const results: object[] = new Array(jobs.length), cpu: object[] = [];
  let inflight = 0, stopCpu = false, leads = false;
  const lead: Record<string, number> = {};
  const cpuLoop = (async () => {
    while (!stopCpu) {
      const t = Date.now();
      cpu.push({ ...(await cpuAll().catch((e) => ({ err: String(e) }))), tRunner: t, leads, inflight });
      await sleep(2000);
    }
  })();
  const c0 = await cpuAll();
  const t0 = Date.now();
  // --pad: fill every job to `cap` pairs with the most recently pushed other refs, overlapping or not, so each push
  // costs the full PREDICT_PAIRS budget. Padded pairs measure CPU only and are left out of the verdict counts.
  const pad = process.argv.includes("--pad"), recent = new Map<string, string>();
  const runOne = async (k: number, j: Job) => {
    const ranked = order === "recent" ? [...j.others].sort((a, b) => b.t - a.t) : j.others;
    const sent = ranked.slice(0, cap), past = ranked.slice(cap);
    const fill = pad ? [...recent].reverse().filter(([r]) => r !== j.ref && !j.others.some((o) => o.ref === r)).slice(0, cap - sent.length) : [];
    recent.delete(j.ref); recent.set(j.ref, j.head);
    const tPush = Date.now();
    inflight++;
    let r: any;
    try {
      r = sent.length + fill.length ? await call("/predict", { shard: shardOf(j.ref), base: j.base, head: j.head, others: [...sent.map((o) => o.head), ...fill.map(([, h]) => h)], budget: PREDICT_CPU, vcpu: VCPU }) : { verdicts: [] };
    } catch (e) { r = { error: String(e), verdicts: [] }; }
    inflight--;
    results[k] = { i: j.i, ref: j.ref, head: j.head, live: j.live, overlapping: j.others.length, sent: sent.length, padded: fill.length, ms: Date.now() - tPush,
      // A job counts as arriving while the site leads if it is in the window or the shard answered paused (a push sent
      // just before the switch can reach the shard just after it).
      leads: (leadIdx >= 0 && k >= leadIdx && k < leadIdx + leadJobs) || !!r.paused, paused: !!r.paused, error: r.error, cpuUsec: r.cpuUsec, wallMs: r.wallMs, headMs: r.headMs,
      pairs: sent.map((o, x) => ({ other: o.ref, n: o.n, truth: o.truth, landedTruth: o.landedTruth, v: r.verdicts?.[x]?.v ?? "Missing", files: r.verdicts?.[x]?.files }))
        .concat(past.map((o) => ({ other: o.ref, n: o.n, truth: o.truth, landedTruth: o.landedTruth, v: "Stale", files: undefined }))) };
  };
  const pending: Promise<void>[] = [];
  for (let k = 0; k < jobs.length; k++) {
    if (k === leadIdx) {
      await setLeads(true); leads = true;
      const tl = Date.now();
      while (inflight > 0) await sleep(50);
      lead.drainMs = Date.now() - tl;
      lead.runsBefore = (await cpuAll()).runs;
    }
    if (k === leadIdx + leadJobs) {
      while (inflight > 0) await sleep(50);
      lead.runsAfter = (await cpuAll()).runs;
      await setLeads(false); leads = false;
    }
    const due = t0 + (k / rate) * 1000;
    if (due > Date.now()) await sleep(due - Date.now());
    pending.push(runOne(k, jobs[k]));
    if (k % 250 === 0) console.log(`${k}/${jobs.length} inflight ${inflight} ${((Date.now() - t0) / 1000).toFixed(0)} s`);
  }
  await Promise.all(pending);
  const wallS = (Date.now() - t0) / 1000, c1 = await cpuAll();
  stopCpu = true; await cpuLoop;
  mkdirSync(`${ROOT}results/raw`, { recursive: true });
  const summary = { shards: shards.length, ...summarize(results as any[], { rate, cap, order, wallS, c0, c1, lead, vcpu: VCPU * shards.length }),
    perShardCpuShare: c1.per.map((u: number, x: number) => +((u - c0.per[x]) / 1e6 / wallS / VCPU).toFixed(3)), gitErrors: c1.errs - c0.errs };
  writeFileSync(`${ROOT}results/raw/${out}.json`, JSON.stringify({ summary, cpu, jobs: results }));
  console.log(JSON.stringify(summary, null, 1));
}

function summarize(rs: any[], m: { rate: number; cap: number; order: string; wallS: number; c0: any; c1: any; lead: Record<string, number>; vcpu: number }) {
  const live = rs.filter((r) => !r.leads), withPairs = live.filter((r) => r.sent + (r.padded ?? 0) > 0);
  const pairs = live.flatMap((r) => r.pairs), sent = pairs.filter((p) => p.v !== "Stale");
  const cpuS = (m.c1.usec - m.c0.usec) / 1e6, runs = m.c1.runs - m.c0.runs;
  const ms = withPairs.map((r) => r.ms);
  return {
    rate: m.rate, cap: m.cap, order: m.order, jobs: rs.length, jobsWithPairs: withPairs.length, wallS: +m.wallS.toFixed(1),
    pairsOverlapping: pairs.length, pairsSent: sent.length, pairsPadded: live.reduce((a, r) => a + (r.padded ?? 0), 0),
    truthConflicts: pairs.filter((p) => p.truth === "Conflict").length,
    found: pairs.filter((p) => p.truth === "Conflict" && p.v === "Conflict").length,
    missedPastCap: pairs.filter((p) => p.truth === "Conflict" && p.v === "Stale").length,
    falseConflicts: sent.filter((p) => p.v === "Conflict" && p.truth !== "Conflict").length,
    // Under the earlier landed-tree method (wp16/predict.py), kept for comparison: conflicts it called that the queue
    // merges clean.
    landedTruthConflicts: pairs.filter((p) => p.landedTruth === "Conflict").length,
    mismatches: sent.filter((p) => p.v !== p.truth).length,
    errors: rs.filter((r) => r.error).length + sent.filter((p) => p.v.startsWith("Err") || p.v === "Missing").length,
    pushToVerdictMs: { p50: pct(ms, 0.5), p95: pct(ms, 0.95), p99: pct(ms, 0.99), max: pct(ms, 1) },
    containerCpuS: +cpuS.toFixed(1), mergeTrees: runs, landedCacheHits: m.c1.hits - m.c0.hits,
    cpuMsPerMergeTree: +(cpuS * 1000 / Math.max(1, runs)).toFixed(1), shardCpuShare: +(cpuS / m.wallS / m.vcpu).toFixed(3),
    lead: { ...m.lead, jobsWhileLeading: rs.filter((r) => r.leads).length, pausedReplies: rs.filter((r) => r.leads && r.paused).length,
      mergeTreesWhileLeading: m.lead.runsAfter - m.lead.runsBefore },
  };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const cmd = process.argv[2];
  if (cmd === "upload") await upload(process.argv[3]);
  else if (cmd === "load-all") console.log(JSON.stringify(await Promise.all(Array.from({ length: Number(arg("shards", "7")) }, (_, x) =>
    call("/load", { shard: `s${x}`, net: "linux" }, 900_000).catch((e) => ({ shard: x, error: String(e) }))))));
  else if (cmd === "stop-all") console.log(JSON.stringify(await Promise.all(Array.from({ length: Number(arg("shards", "7")) }, (_, x) => call("/stop", { shard: `s${x}` })))));
  else if (cmd === "load") console.log(JSON.stringify(await call("/load", { shard: arg("shard", "s0"), net: "linux" }, 900_000)));
  else if (cmd === "stop") console.log(JSON.stringify(await call("/stop", { shard: arg("shard", "s0") })));
  else if (cmd === "cpu") console.log(JSON.stringify(await call("/cpu", { shard: arg("shard", "s0") })));
  else if (cmd === "replay") await replay(process.argv[3]);
  else throw new Error("usage: node run/wp16.ts upload <dir> | load | stop | cpu | replay <jobs.json> [--rate --cap --order --leads-at --out --limit]");
}
