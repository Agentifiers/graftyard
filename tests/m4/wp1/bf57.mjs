// BF-57 hosted on graft-m4a (build/BUILD-PLAN.md, M4-WP1): one hook timing out at 10 s beside 49 healthy hooks of
// the same owner (m3org), 10 events/s. The hooks are spread over three repositories (20, 20, and 10, because a
// repository holds at most 20), all pointing at the receiver Worker `graft-m4-hooksink`, whose `/h/<n>?slow=1` answers
// after 11 s. Each event is a commit status (`POST /repos/{o}/{r}/statuses/{sha}`, one `status` webhook) whose
// description carries a sequence number per repository, so the receiver's log shows each hook's order.
//
// Measures, from the receiver's log and the edge's replies:
// - healthy first-attempt latency: arrival at the receiver minus the status's 201 (an upper bound on entry commit to
//   attempt start, plus the request's one-way time), p99 over every healthy delivery;
// - the slow hook's deliveries in chain order (its sequence numbers strictly increase);
// - the Webhooks cursor lag, from the commit of the first entry a cursor commit passes to that cursor's own commit,
//   which each repository leader logs ("webhooks cursor <repo> read <i> lag <ms>ms"), read from Workers Logs over the
//   three site scripts (the tools edge reads no repository chain on a product install).
//
// A status write the edge sheds (503 with `Retry-After`) is sent again after it, as a client does, so the load
// commits at 10 events/s; latency is measured from the write's 201.
//
//   source tests/am1/lf13/env-m4a.sh; node tests/m4/wp1/bf57.mjs <out.jsonl> [seconds=60]
import { api, log, row, sleep, tokens } from "../../am1/hosted/lib.mjs";

const SINK = process.env.HOOKSINK ?? "https://graft-m4-hooksink.<workers-subdomain>.workers.dev";
const [out, secs = "60"] = process.argv.slice(2);
if (!out) throw new Error("usage: bf57.mjs <out.jsonl> [seconds]");
const T = tokens.admin;
const REPOS = [["j4", 20], ["bf57b", 20], ["bf57c", 10]];
const RATE = 10;

async function ensureRepo(name) {
  const r = await api("GET", `/api/v3/repos/m3org/${name}`, T);
  if (r.status === 200) return r.body.id;
  const c = await api("POST", "/api/v3/orgs/m3org/repos", T, { name, auto_init: false });
  if (c.status !== 201) throw new Error(`create ${name}: ${c.status} ${JSON.stringify(c.body).slice(0, 200)}`);
  return c.body.id;
}

/** The repository's hooks, made fresh: any it has are deleted, then `n` are created, all at the receiver as
 * `/h/<base + i>`; j4's first is the slow one. */
async function freshHooks(name, n, base, first) {
  const have = await api("GET", `/api/v3/repos/m3org/${name}/hooks`, T);
  if (have.status !== 200) throw new Error(`hooks ${name}: ${have.status}`);
  // `KEEP_HOOKS=1` reuses a previous run's hooks when they are the same set.
  const want = Array.from({ length: n }, (_, i) => `${SINK}/h/${base + i}${first && i === 0 ? "?slow=1" : ""}`);
  const urls = have.body.map((h) => h.config.url).sort();
  if (process.env.KEEP_HOOKS && JSON.stringify(urls) === JSON.stringify([...want].sort())) {
    return want.map((u) => have.body.find((h) => h.config.url === u).id);
  }
  for (const h of have.body) await api("DELETE", `/api/v3/repos/m3org/${name}/hooks/${h.id}`, T);
  const ids = [];
  for (let i = 0; i < n; i++) {
    const slow = first && i === 0;
    const url = `${SINK}/h/${base + i}${slow ? "?slow=1" : ""}`;
    const c = await api("POST", `/api/v3/repos/m3org/${name}/hooks`, T,
      { config: { url, content_type: "json", secret: `bf57-${name}-${i}-secret` }, events: ["status"] });
    if (c.status !== 201) throw new Error(`hook ${name} ${i}: ${c.status} ${JSON.stringify(c.body).slice(0, 200)}`);
    ids.push(c.body.id);
  }
  return ids;
}

const repos = [];
for (const [j, [name, n]] of REPOS.entries()) {
  const id = await ensureRepo(name);
  const hooks = await freshHooks(name, n, 100 * (j + 1), name === "j4");
  repos.push({ name, id, hooks });
  log(`${name} repo:${id} ${hooks.length} hooks`);
}
const slowHook = repos[0].hooks[0];
log(`slow hook ${slowHook}`);
await sleep(process.env.KEEP_HOOKS ? 2_000 : 30_000); // let the pings drain
await fetch(`${SINK}/clear`);

// The load: RATE statuses a second, round robin over the repositories, each with its own sequence.
const sent = new Map(); // `${repo}:${seq}` -> 201 time
const seqs = new Map(repos.map((r) => [r.name, 0]));
const t0 = Date.now();
const end = t0 + Number(secs) * 1000;
let k = 0;
let shed = 0;
const inflight = new Set();
while (Date.now() < end) {
  const r = repos[k % repos.length];
  k++;
  const seq = seqs.get(r.name);
  seqs.set(r.name, seq + 1);
  const p = (async () => {
    let res;
    for (let i = 0; i < 30; i++) {
      res = await api("POST", `/api/v3/repos/m3org/${r.name}/statuses/${"b".repeat(40)}`, T,
        { state: "success", context: "bf57", description: `${r.name} ${seq}` });
      if (res.status !== 503) break;
      shed++;
      await sleep(1000 * Number(res.headers?.["retry-after"] ?? 1));
    }
    const at = Date.now();
    sent.set(`${r.name} ${seq}`, { at, status: res.status });
    row(out, { kind: "sent", repo: r.name, seq, at, status: res.status, ms: res.ms });
  })();
  inflight.add(p);
  p.finally(() => inflight.delete(p));
  await sleep(Math.max(0, t0 + (k * 1000) / RATE - Date.now()));
}
await Promise.all(inflight);
const ok = [...sent.values()].filter((s) => s.status === 201);
log(`sent ${sent.size} statuses, ${ok.length} committed, by ${(Math.max(...ok.map((s) => s.at)) - t0) / 1000} s; ${shed} sheds re-sent`);
// The slow hook takes 11 s per delivery, one at a time: wait for it.
const slowWant = [...sent.keys()].filter((key) => key.startsWith("j4 ")).length;
for (let i = 0; i < 400; i++) {
  const d = await (await fetch(`${SINK}/dump`)).json();
  const n = d.filter((x) => x.event === "status" && Number(x.hook) === slowHook).length;
  if (n >= slowWant) break;
  if (i % 6 === 0) log(`slow hook ${n} of ${slowWant}`);
  await sleep(10_000);
}
const dump = await (await fetch(`${SINK}/dump`)).json();
for (const x of dump) row(out, { kind: "arrival", ...x });
// Cursor lag from the leaders' log lines, through the Workers observability API.
const lags = [];
for (const script of ["graft-m4a-site-a", "graft-m4a-site-b", "graft-m4a-site-c"]) {
  for (let from = t0; from < end + 30_000; from += 30_000) {
    const r = await fetch(`https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/workers/observability/telemetry/query`, {
      method: "POST", headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`, "content-type": "application/json" },
      body: JSON.stringify({ queryId: "bf57", timeframe: { from, to: from + 30_000 }, view: "events", limit: 2000, parameters: { filters: [
        { key: "$metadata.service", operation: "eq", type: "string", value: script },
        { key: "$metadata.message", operation: "includes", type: "string", value: "webhooks cursor" }] } }),
    });
    for (const e of (await r.json()).result?.events?.events ?? []) {
      const m = e.$metadata?.message?.match(/^webhooks cursor (\d+) read (\d+) lag (\d+)ms$/);
      if (!m) continue;
      const repo = repos.find((x) => x.id === Number(m[1]))?.name;
      if (repo) lags.push({ repo, at: e.timestamp - t0, read: Number(m[2]), lag: Number(m[3]) });
    }
  }
}
for (const s of lags) row(out, { kind: "cursor", ...s });
log(`cursor commits logged: ${lags.length}`);
const lagMax = (from, to) => Math.max(0, ...lags.filter((x) => x.at >= from && x.at < to).map((x) => x.lag));
const half = (end - t0) / 2;
log(`cursor lag max: first half ${lagMax(0, half)} ms, second half ${lagMax(half, end - t0)} ms`);

// Healthy first-attempt latency.
const first = new Map();
for (const x of dump.filter((x) => x.event === "status")) {
  const key = `${x.hook} ${x.after}`;
  if (!first.has(key)) first.set(key, x);
}
const healthy = [];
for (const x of first.values()) {
  if (Number(x.hook) === slowHook) continue;
  const s = sent.get(x.after);
  if (s) healthy.push(x.at - s.at);
}
healthy.sort((a, b) => a - b);
const p = (q) => healthy[Math.min(healthy.length - 1, Math.floor(q * healthy.length))];
const want = repos.reduce((n, r) => n + r.hooks.length * seqs.get(r.name), 0) - slowWant;
log(`healthy first attempts ${healthy.length} of ${want}: p50 ${p(0.5)} ms, p99 ${p(0.99)} ms, max ${healthy.at(-1)} ms`);
// The slow hook's order.
const slow = dump.filter((x) => x.event === "status" && Number(x.hook) === slowHook).map((x) => Number(x.after.split(" ")[1]));
const firstSeen = [...new Set(slow)];
const inOrder = firstSeen.every((v, i) => i === 0 || v > firstSeen[i - 1]);
log(`slow hook: ${firstSeen.length} of ${slowWant} delivered, in order: ${inOrder}, attempts ${slow.length}`);
// Every hook's order.
let bad = 0;
for (const r of repos) for (const h of r.hooks) {
  const s = dump.filter((x) => x.event === "status" && Number(x.hook) === h).map((x) => Number(x.after.split(" ")[1]));
  const f = [...new Set(s)];
  if (!f.every((v, i) => i === 0 || v > f[i - 1])) bad++;
}
log(`hooks out of order: ${bad}`);
row(out, { kind: "summary", shed, committed: ok.length, lag1: lagMax(0, half), lag2: lagMax(half, end - t0), healthy: healthy.length, want, p50: p(0.5), p99: p(0.99), max: healthy.at(-1), slow: firstSeen.length, slowWant, inOrder, bad });
