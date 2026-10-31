// M0-WP6, B0-23: clone cache under CI load (src/rehydrate.ts, "B0-23"). Load clients are `Shard` objects created with
// location hints, each running `rate` clones a second from its alarm against the script's public `/cc` route, while the
// tip they want moves every `--every` ms. A hit reads one of 8 replicas from R2; a miss goes to the network's shard,
// which runs one upload-pack per key and fills the cache.
//
//   node run/clone-cache.ts mono [--files 20000 --commits 300]       build the monorepo on the shard (once)
//   node run/clone-cache.ts run --run X [--clients 11 --rate 3 --seconds 600 --every 6000 --lag 0 --lag-max 0 --read 1048576]
//     --lag/--lag-max: each clone wants the tip as of a uniform random delay in that range, the time from a push to its
//     CI job's checkout. 0/0 is the worst case: every clone wants the tip the moment it exists.
//     --claim: a miss makes a create-only claim object; only its creator calls the shard, the rest wait for the bucket.
//     --prefill: client 0 also asks the shard to fill each tip's entry the moment the tip appears (a push hook).
//   node run/clone-cache.ts collect --run X

import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, options: {
  files: { type: "string", default: "20000" }, commits: { type: "string", default: "300" },
  run: { type: "string" }, clients: { type: "string", default: "11" }, rate: { type: "string", default: "3" },
  seconds: { type: "string", default: "600" }, every: { type: "string", default: "6000" }, lag: { type: "string", default: "0" }, "lag-max": { type: "string" }, prefill: { type: "boolean", default: false }, claim: { type: "boolean", default: false },
  read: { type: "string", default: "1048576" }, replicas: { type: "string", default: "8" },
  hints: { type: "string", default: "enam,wnam,weur" }, partial: { type: "boolean", default: false } } });
const ROOT = new URL("../", import.meta.url).pathname;
const URL_ = "https://graft-m0-rehydrate.<workers-subdomain>.workers.dev/";
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const MONO = `${ROOT}.gen/b0-23-mono.json`;
const call = async (body: object) => {
  const r = await fetch(URL_, { method: "POST", headers: { authorization: `Bearer ${TOKEN}` }, body: JSON.stringify(body) });
  if (!r.ok) throw new Error(`${r.status} ${await r.text()}`);
  return r.json();
};
const cfgPath = (run: string) => `${ROOT}results/raw/b0-23-${run}.cfg.json`;

type Row = { t: number; idx: number; status: number; cc?: string; r2Ms?: number; ttfb?: number; bytes?: number; ms: number; error?: string };
const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : NaN; };

const cmd = positionals[0];
if (cmd === "mono") {
  const r = await call({ op: "mono", net: "mono", files: Number(opt.files), commits: Number(opt.commits), hint: "enam" });
  writeFileSync(MONO, JSON.stringify(r));
  console.log({ ...r, tips: r.tips.length });
} else if (cmd === "run") {
  const run = opt.run!;
  if (existsSync(cfgPath(run))) throw new Error(`run ${run} exists`);
  const { tips } = JSON.parse(readFileSync(MONO, "utf8")) as { tips: string[] };
  const cfg = { net: "mono", tips: tips.slice(1), run, replicas: Number(opt.replicas), read: Number(opt.read), rate: Number(opt.rate),
    seconds: Number(opt.seconds), start: Date.now() + 20_000, everyMs: Number(opt.every), lagMs: Number(opt.lag),
    lagMaxMs: Number(opt["lag-max"] ?? opt.lag), offset: Math.ceil(Number(opt["lag-max"] ?? opt.lag) / Number(opt.every)), prefill: opt.prefill, claim: opt.claim };
  if (cfg.offset + cfg.seconds * 1000 / cfg.everyMs > cfg.tips.length) throw new Error(`only ${cfg.tips.length} tips for ${cfg.seconds} s`);
  const spec = { clients: Number(opt.clients), hints: opt.hints!.split(",") };
  mkdirSync(`${ROOT}results/raw`, { recursive: true });
  writeFileSync(cfgPath(run), JSON.stringify({ ...spec, cfg: { ...cfg, tips: cfg.tips.length } }));
  console.log(await call({ op: "ci", net: "mono", ...spec, cfg }));
  console.log(`started; collect after ${new Date(cfg.start + cfg.seconds * 1000 + 30_000).toISOString()}`);
} else if (cmd === "collect") {
  const run = opt.run!;
  const spec = JSON.parse(readFileSync(cfgPath(run), "utf8"));
  const res = await call({ op: "ciResult", net: "mono", clients: spec.clients, hints: spec.hints, cfg: { run } }) as ({ colo: string; rows: Row[] } | null)[];
  const missing = res.flatMap((r, i) => r ? [] : [i]);
  if (missing.length && !opt.partial) throw new Error(`clients ${missing} not finished (--partial to skip them)`);
  const fills = (await call({ op: "fills", net: "mono" }) as { at: number; genMs: number; firstMs: number; putMs: number; size: number }[])
    .filter((f) => f.at >= spec.cfg.start && f.at <= spec.cfg.start + spec.cfg.seconds * 1000 + 60_000);
  const all = res.filter((r) => r).flatMap((r) => r!.rows.map((x) => ({ ...x, colo: r!.colo })));
  // Rows with status -1 are slots a late client skipped: not clones, so they count in neither hits nor failures.
  const rows = all.filter((r) => r.status !== -1);
  const steadyFrom = Number(spec.cfg.lagMaxMs ?? 0) + 30_000;
  const ok = rows.filter((r) => r.status === 200 && (r.ttfb ?? -1) >= 0);
  const by = (cc: string) => ok.filter((r) => r.cc === cc);
  const stat = (rs: Row[]) => ({ n: rs.length, ttfbP50: pct(rs.map((r) => r.ttfb!), 0.5), ttfbP99: pct(rs.map((r) => r.ttfb!), 0.99),
    r2P50: pct(rs.map((r) => r.r2Ms!), 0.5), r2P99: pct(rs.map((r) => r.r2Ms!), 0.99) });
  const summary = {
    run, cfg: spec.cfg, clients: spec.clients, unfinished: missing, colos: Object.fromEntries(Object.entries(Object.groupBy(res.filter((r) => r), (r) => r!.colo)).map(([k, v]) => [k, v!.length])),
    clones: rows.length, skipped: all.length - rows.length, failed: rows.length - ok.length, errors: rows.filter((r) => r.status !== 200).slice(0, 5),
    rate: rows.length / spec.cfg.seconds / ((spec.clients - missing.length) / spec.clients),
    // r2Hit: served from the bucket. coalesced: no upload-pack of its own (bucket, or the shard's in-flight or finished
    // stream for the key). The design's "each new tip misses once" is the second.
    r2Hit: (by("hit").length + by("hit0").length) / rows.length, waited: by("waited").length / rows.length, coalesced: (rows.length - by("gen").length - (rows.length - ok.length)) / rows.length,
    tipsServed: new Set(ok.map((r) => r.idx)).size,
    // After the first lag window plus 30 s, the cache is in its steady state: the run opens with every client wanting
    // tips that do not exist in the cache yet, which a running install never sees.
    steady: (() => { const st = rows.filter((r) => r.t >= steadyFrom); const so = st.filter((r) => r.status === 200 && (r.ttfb ?? -1) >= 0);
      return { from: steadyFrom, clones: st.length, failed: st.length - so.length, r2Hit: so.filter((r) => r.cc?.startsWith("hit")).length / st.length, waited: so.filter((r) => r.cc === "waited").length / st.length,
        coalesced: (so.length - so.filter((r) => r.cc === "gen").length) / st.length, ...stat(so) }; })(),
    all: stat(ok), hits: stat(by("hit")), hit0: stat(by("hit0")), waited: stat(by("waited")), gen: stat(by("gen")), follow: stat(by("follow")), done: stat(by("done")),
    fills: fills.length, fillGenP50: pct(fills.map((f) => f.genMs), 0.5), fillFirstP50: pct(fills.map((f) => f.firstMs), 0.5), fillPutP50: pct(fills.map((f) => f.putMs), 0.5),
    fillPutP99: pct(fills.map((f) => f.putMs), 0.99),
  };
  writeFileSync(`${ROOT}results/raw/b0-23-${run}.json`, JSON.stringify({ summary, rows }));
  console.log(JSON.stringify(summary, null, 1));
} else {
  console.log("usage: mono | run --run X | collect --run X");
}
