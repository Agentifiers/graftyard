// BF-58 hosted on graft-m4a (build/BUILD-PLAN.md, M4-WP1): kill the site holding a hook owner's `HookSequencer`
// with deliveries pending. Every event in the range must reach every hook at least once, with unchanged GUIDs.
//
// The owner is m3org (record home: site 0, `enam`), so its `HookSequencer` is `hookseq:7100001` on site a. The run:
//  1. three repositories of m3org with five hooks each at the receiver Worker; on the second, hook 0 answers after
//     11 s (`?slow=1`), so its queue keeps a backlog and site a's store holds pending deliveries at the kill;
//  2. statuses at 2/s, round robin, for the whole run (`LOAD_S`);
//  3. at `KILL_AT_S`, the operator runs the kill (`tests/m4/wp1/bf58-kill.sh down`: a site day of site a alone with
//     `GRAFTYARD_SITE_DOWN=1`), and at `KILL_AT_S + DOWN_S` the restore (`bf58-kill.sh up`, which also discards
//     every `HookSequencer` store on site a through `GRAFTYARD_HOOKSEQ_RESET_BEFORE`); this script only marks the
//     times it is told through the files /tmp/bf58.down and /tmp/bf58.up;
//  4. after the load, it waits for the slow hook to drain, then checks the receiver's log.
//
// Pass: every (status, hook) pair the edge acknowledged (201) arrived at least once, and every arrival of one pair
// carries one GUID. Also reported: deliveries more than once, the outage-to-first-delivery time, and the
// outage-to-first-redelivery time after the restore.
//
//   source tests/am1/lf13/env-m4a.sh; node tests/m4/wp1/bf58.mjs <out.jsonl>
import { existsSync, readFileSync } from "node:fs";
import { api, log, row, sleep, tokens } from "../../am1/hosted/lib.mjs";

const SINK = process.env.HOOKSINK ?? `https://graft-m4-hooksink.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const [out] = process.argv.slice(2);
if (!out) throw new Error("usage: bf58.mjs <out.jsonl>");
const T = tokens.admin;
const LOAD_S = Number(process.env.LOAD_S ?? 900);
const RATE = 2;
const REPOS = ["bf58a", "bf58b", "bf58c"];
const HOOKS = 5;
// The slow hook's delay: `1` is the receiver's 11 s; a number is milliseconds. 3 s keeps a backlog at 2/s over three
// repositories (one status in three reaches it) that drains within the driver's wait.
const SLOW = process.env.SLOW_MS ?? "1";
// Each description ends with this run's tag, and only arrivals carrying it count, so a previous run's deliveries still
// draining to the same hooks are not read as this run's.
const RUN = Date.now().toString(36);
const mine = (x) => typeof x.after === "string" && x.after.endsWith(` ${RUN}`);

async function ensureRepo(name) {
  const r = await api("GET", `/api/v3/repos/m3org/${name}`, T);
  if (r.status === 200) return r.body.id;
  const c = await api("POST", "/api/v3/orgs/m3org/repos", T, { name });
  if (c.status !== 201) throw new Error(`create ${name}: ${c.status}`);
  return c.body.id;
}
async function hooks(name, base, slow) {
  const have = await api("GET", `/api/v3/repos/m3org/${name}/hooks`, T);
  const want = Array.from({ length: HOOKS }, (_, i) => `${SINK}/h/${base + i}${slow && i === 0 ? `?slow=${SLOW}` : ""}`);
  if (JSON.stringify(have.body.map((h) => h.config.url).sort()) === JSON.stringify([...want].sort())) {
    return want.map((u) => have.body.find((h) => h.config.url === u).id);
  }
  for (const h of have.body) await api("DELETE", `/api/v3/repos/m3org/${name}/hooks/${h.id}`, T);
  const ids = [];
  for (const url of want) {
    const c = await api("POST", `/api/v3/repos/m3org/${name}/hooks`, T,
      { config: { url, content_type: "json", secret: `bf58-${url.slice(-6)}-secret` }, events: ["status"] });
    if (c.status !== 201) throw new Error(`hook ${url}: ${c.status}`);
    ids.push(c.body.id);
  }
  return ids;
}

// The receiver's log, read again after a failed connection.
async function sinkDump() {
  for (let i = 0; ; i++) {
    try {
      return await (await fetch(`${SINK}/dump`)).json();
    } catch (e) {
      if (i >= 5) throw e;
      await sleep(5_000);
    }
  }
}

const repos = [];
for (const [j, name] of REPOS.entries()) {
  repos.push({ name, id: await ensureRepo(name), hooks: await hooks(name, 1000 + 10 * j, j === 1) });
  log(`${name} ${repos.at(-1).hooks.length} hooks`);
}
const slowHook = repos[1].hooks[0];
await sleep(20_000);
await fetch(`${SINK}/clear`);

const sent = new Map();
const t0 = Date.now();
const end = t0 + LOAD_S * 1000;
let k = 0;
const inflight = new Set();
const marks = {};
while (Date.now() < end) {
  for (const m of ["down", "dead", "up"]) {
    if (!marks[m] && existsSync(`/tmp/bf58.${m}`)) {
      marks[m] = Number(readFileSync(`/tmp/bf58.${m}`, "utf8").trim()) || Date.now();
      log(`mark ${m} at +${(marks[m] - t0) / 1000} s`);
      row(out, { kind: "mark", m, at: marks[m] });
    }
  }
  const r = repos[k % repos.length];
  const seq = Math.floor(k / repos.length);
  k++;
  const p = (async () => {
    const res = await api("POST", `/api/v3/repos/m3org/${r.name}/statuses/${"d".repeat(40)}`, T,
      { state: "success", context: "bf58", description: `${r.name} ${seq} ${RUN}` });
    const at = Date.now();
    if (res.status === 201) sent.set(`${r.name} ${seq} ${RUN}`, at);
    row(out, { kind: "sent", repo: r.name, seq, at, status: res.status, ms: res.ms });
  })();
  inflight.add(p);
  p.finally(() => inflight.delete(p));
  await sleep(Math.max(0, t0 + (k * 1000) / RATE - Date.now()));
}
await Promise.all(inflight);
log(`acknowledged ${sent.size} of ${k} statuses`);

const want = new Set();
for (const r of repos) for (const key of sent.keys()) if (key.startsWith(`${r.name} `)) for (const h of r.hooks) want.add(`${h}|${key}`);
let dump = [];
for (let i = 0; i < 360; i++) {
  dump = (await sinkDump()).filter((x) => x.event === "status" && mine(x));
  const got = new Set(dump.map((x) => `${x.hook}|${x.after}`));
  const missing = [...want].filter((w) => !got.has(w));
  if (missing.length === 0) break;
  if (i % 6 === 0) log(`${missing.length} pairs not yet delivered (${missing.filter((m) => m.startsWith(`${slowHook}|`)).length} of them the slow hook's)`);
  await sleep(10_000);
}
for (const x of dump) row(out, { kind: "arrival", ...x });
const got = new Map();
for (const x of dump) {
  const key = `${x.hook}|${x.after}`;
  if (!got.has(key)) got.set(key, new Set());
  got.get(key).add(x.guid);
}
const missing = [...want].filter((w) => !got.has(w));
const twoGuids = [...got.entries()].filter(([, g]) => g.size > 1);
const counts = new Map();
for (const x of dump) counts.set(`${x.hook}|${x.after}`, (counts.get(`${x.hook}|${x.after}`) ?? 0) + 1);
const dups = [...counts.values()].filter((n) => n > 1).length;
// The outage counts from `dead`, when Workers Logs first showed site a's `HookSequencer` on the down version (a
// running object keeps its old version for minutes after a deploy), else from the deploy.
const killed = marks.dead ?? marks.down;
const afterDown = killed ? dump.filter((x) => x.at > killed) : [];
const afterUp = marks.up ? dump.filter((x) => x.at > marks.up) : [];
const firstDuring = afterDown.find((x) => !marks.up || x.at < marks.up);
const firstAfter = afterUp[0];
const redelivered = afterUp.find((x) => (counts.get(`${x.hook}|${x.after}`) ?? 0) > 1);
log(`pairs ${want.size}: delivered ${want.size - missing.length}, missing ${missing.length}, under two GUIDs ${twoGuids.length}, delivered more than once ${dups}`);
log(`first delivery during the outage: ${firstDuring ? (firstDuring.at - killed) / 1000 + " s after the kill" : "none"}`);
log(`first delivery after the restore: ${firstAfter ? (firstAfter.at - marks.up) / 1000 + " s after up" : "none"}; first redelivery ${redelivered ? (redelivered.at - marks.up) / 1000 + " s after up" : "none"}`);
row(out, { kind: "summary", pairs: want.size, missing: missing.length, twoGuids: twoGuids.length, dups, marks,
  firstDuring: firstDuring ? firstDuring.at - killed : null, firstAfter: firstAfter ? firstAfter.at - marks.up : null,
  firstRedelivery: redelivered ? redelivered.at - marks.up : null, missingSample: missing.slice(0, 10) });
