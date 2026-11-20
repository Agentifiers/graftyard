// The orb's side of Load34 (tests/am1/load34/load34.mjs): the load script's control calls, the tip:move pack stager,
// and each client's CPU per operation from the account's analytics. a1.mjs --hosted N uses the calls; b034.sh runs the
// stager before the row and the CPU read after it:
//   node tests/am1/load34/driver.mjs stage B034_JSON      # each future commit's thin pack, built from the bare clone
//   node tests/am1/load34/driver.mjs cpu RUN FROM TO      # per-client CPU and active time from durableObjectsPeriodicGroups
// The load script's bearer token is LOAD34_TOKEN_FILE (default $M3_STATE/load34.token, written by deploy.sh); it goes
// only in an `authorization` header. The Cloudflare API token (the CPU read, a driver tool) only as a header too.
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { gunzipSync } from "node:zlib";

export const LOAD = process.env.LOAD34_URL ?? `https://graft-m1-load.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const tokenFile = () => process.env.LOAD34_TOKEN_FILE ?? `${process.env.M3_STATE ?? `${process.env.HOME}/.graft-m3h`}/load34.token`;
export const loadToken = () => readFileSync(tokenFile(), "utf8").trim();

/** The R2 prefix of a history's staged tip:move packs: one per list of future commits. */
export const packsPrefix = (future) => `tip/${createHash("sha256").update(future.join("\n")).digest("hex").slice(0, 16)}`;

async function call(path, body, { method = "POST", raw = false, timeout = 60_000 } = {}) {
  for (let i = 0; ; i++) {
    try {
      const r = await fetch(`${LOAD}${path}`, { method, body: body === undefined || Buffer.isBuffer(body) ? body : JSON.stringify(body),
        headers: { authorization: `Bearer ${loadToken()}`, "user-agent": "graftyard-am1" }, signal: AbortSignal.timeout(timeout) });
      if (raw) return r;
      const t = await r.text();
      if (!r.ok) throw new Error(`${path} ${r.status} ${t.slice(0, 200)}`);
      return JSON.parse(t);
    } catch (e) {
      if (i >= 3) throw e;
      await new Promise((res) => setTimeout(res, 2000 * (i + 1)));
    }
  }
}
export const start = (spec, hints) => call("/start", { spec, hints }, { timeout: 120_000 });
export const progress = (run, n, members) => call("/progress", { run, n, members }, { timeout: 60_000 });
export const stop = (run, n) => call("/stop", { run, n });
export const forget = (run, n) => call("/forget", { run, n });
export const purge = (prefix) => call("/purge", { prefix });
/** One client's chunks from R2, in order: `[{seq, done, samples, pushes, acks, seen, summary?, misses?}]`. */
export async function collect(run, i) {
  const { keys } = await call(`/list?prefix=${encodeURIComponent(`runs/${run}/${i}/`)}`, undefined, { method: "GET" });
  const out = [];
  for (const key of keys.sort()) {
    const r = await call(`/get?key=${encodeURIComponent(key)}`, undefined, { method: "GET", raw: true, timeout: 120_000 });
    if (!r.ok) throw new Error(`collect ${key}: ${r.status}`);
    out.push(JSON.parse(gunzipSync(Buffer.from(await r.arrayBuffer())).toString()));
  }
  return out;
}

/** Stages each future commit's thin pack against its first parent (what `git push` sends a receiver at that parent),
 *  `<prefix>/<i>`, skipping keys already staged. Returns `{n, staged, bytes, maxBytes}`. */
export async function stagePacks(bare, future, log = console.log) {
  const prefix = packsPrefix(future);
  let staged = 0, bytes = 0, maxBytes = 0;
  for (const [i, c] of future.entries()) {
    const key = `${prefix}/${i}`;
    const head = await call(`/stage?key=${encodeURIComponent(key)}`, undefined, { method: "HEAD", raw: true });
    if (head.status === 200) continue;
    const p = spawnSync("git", ["pack-objects", "--stdout", "--thin", "--revs", "--delta-base-offset", "-q"],
      { cwd: bare, input: `${c}\n^${c}^1\n`, maxBuffer: 1 << 30 });
    if (p.status !== 0) throw new Error(`pack-objects ${i}: ${p.stderr.toString().slice(0, 200)}`);
    await call(`/stage?key=${encodeURIComponent(key)}`, p.stdout, { method: "PUT", timeout: 300_000 });
    staged++; bytes += p.stdout.length; maxBytes = Math.max(maxBytes, p.stdout.length);
    if (staged % 100 === 0) log(`staged ${staged} (${i + 1} of ${future.length})`);
  }
  return { prefix, n: future.length, staged, bytes, maxBytes };
}

/** Each client object's CPU and active time over [from, to) (ISO), from `durableObjectsPeriodicGroups` by object name
 *  (`<run>-<i>`), joined to the client's request count from its output: CPU ms per operation per client and in all. */
export async function clientCpu(run, from, to, requestsByClient) {
  const acct = process.env.CLOUDFLARE_ACCOUNT_ID, H = { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}`, "content-type": "application/json" };
  const ns = (await (await fetch(`https://api.cloudflare.com/client/v4/accounts/${acct}/workers/durable_objects/namespaces?per_page=1000`, { headers: H })).json())
    .result.filter((x) => x.script === "graft-m1-load" && x.class === "Load34").map((x) => x.id);
  const q = `query($a: String!, $s: Time!, $e: Time!, $n: [String!]) { viewer { accounts(filter: {accountTag: $a}) {
    p: durableObjectsPeriodicGroups(limit: 10000, filter: {datetime_geq: $s, datetime_lt: $e, namespaceId_in: $n}) {
      sum { cpuTime activeTime } dimensions { name } } } } }`;
  const v = await (await fetch("https://api.cloudflare.com/client/v4/graphql", { method: "POST", headers: H,
    body: JSON.stringify({ query: q, variables: { a: acct, s: from, e: to, n: ns } }) })).json();
  if (v.errors) throw new Error(JSON.stringify(v.errors).slice(0, 300));
  return cpuLine(run, v.data.viewer.accounts[0].p, requestsByClient);
}

/** The CPU line from analytics rows `{sum: {cpuTime µs, activeTime µs}, dimensions: {name}}` and each client's requests
 *  (`{i: {api, git, member}}`): per client CPU ms per operation, and the run's total CPU-s, GB-s at 128 MB, and USD at
 *  $0.02 per million CPU-ms and $12.50 per million GB-s (research/prices.json, cf.do.duration). */
export function cpuLine(run, rows, requestsByClient) {
  const by = {};
  for (const r of rows) {
    const m = r.dimensions.name?.match(new RegExp(`^${run}-(\\d+)$`));
    if (!m) continue;
    const c = (by[m[1]] ??= { cpuMs: 0, activeS: 0 });
    c.cpuMs += r.sum.cpuTime / 1000; c.activeS += r.sum.activeTime / 1e6;
  }
  const clients = Object.entries(by).map(([i, c]) => {
    const ops = Object.values(requestsByClient[i] ?? {}).reduce((a, b) => a + b, 0);
    return { i: Number(i), ops, cpuMs: Math.round(c.cpuMs), cpuMsPerOp: ops ? +(c.cpuMs / ops).toFixed(3) : null, activeS: Math.round(c.activeS) };
  }).sort((a, b) => a.i - b.i);
  const cpuMs = clients.reduce((a, c) => a + c.cpuMs, 0), ops = clients.reduce((a, c) => a + c.ops, 0);
  const gbS = clients.reduce((a, c) => a + c.activeS, 0) * 0.128;
  return { clients, cpuS: +(cpuMs / 1000).toFixed(1), ops, cpuMsPerOp: ops ? +(cpuMs / ops).toFixed(3) : null, gbS: Math.round(gbS),
    usd: +((cpuMs * 0.02 + gbS * 12.5) / 1e6).toFixed(4) };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [cmd, ...a] = process.argv.slice(2);
  if (cmd === "stage") {
    const b = JSON.parse(readFileSync(a[0], "utf8"));
    console.log(JSON.stringify({ step: "load34 stage", ...(await stagePacks(b.bare, b.future)) }));
  } else if (cmd === "cpu") {
    // The a1 rows file's hosted line gives each client's requests.
    const rows = readFileSync(a[3], "utf8").trim().split("\n").map((l) => JSON.parse(l));
    const h = rows.filter((r) => r.hosted?.clients).pop()?.hosted;
    const req = Object.fromEntries((h?.clients ?? []).map((c) => [c.i, c.requests ?? {}]));
    console.log(JSON.stringify({ step: "load34 cpu", run: a[0], ...(await clientCpu(a[0], a[1], a[2], req)) }));
  } else {
    console.error("usage: driver.mjs stage B034_JSON | cpu RUN FROM TO A1_ROWS");
    process.exit(2);
  }
}
