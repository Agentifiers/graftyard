// B0-30's replay: 30 simulated days of journey 9 on m3org/b0-30 in about 40 h (SPEED 18), with journey 9's log bytes
// and agent rows per simulated day (tests/m0/results/m0-wp9.md, B0-30), and these substitutions
// (tests/am1/results/b0-30.md, "Substitutions"):
//
// - **Pushes.** Journey 9's 2.48 GB of inline packs a simulated day go as real git pushes through graft-m1's edge by
//   m3-admin to refs/heads/b030/<i> (Human class), PUSH_PER_S a second, evenly spaced (default 1.6, under `hot`'s 2 a
//   second so admission keeps them inline up to INLINE_MAX), PUSH_BYTES each so the bytes a day are journey 9's.
//   Journey 9's own count (4.17 a second, 75 at 18x) is not reachable, and its agent-class pushes and every edge route
//   that reads the repository's tiers (`{"rows": true}`: agent pushes' `push_context`, the task and lease API, GraphQL)
//   took the leader past its 300 s CPU limit on this 4 GB tier. Each blob is a fixed random base with 64 bytes
//   replaced, so the git cache's repacks and promotions delta them. From 10:30Z the service runs with
//   B030_PACK_GB_DAY=0.31 (journey 9's 40 KB mean pack at 1.6 pushes/s): at 2.48 GB a simulated day the log took in
//   about 0.8 MB/s and the archive alarm drained about 0.2 MB/s under load, so the log grew past its cap. From
//   12:10Z it runs with B030_PUSHES=0: every push-path stall was in the git cache shard, so the log takes only the
//   recorded agent rows, about 0.14 GB a simulated day, and the archive alarm holds it at LOG_MAX_BYTES.
// - **Agent rows.** M0's rows a simulated day (6,050 tasks, each with a task row, a 2 KB PR opened on a pushed branch
//   at its tip, and a 16 KB `PathSet` deleted with the PR 20 simulated minutes later; 48,400 300-byte statuses; 60,500
//   150-byte provenance rows) are recorded by the leader in batched entries (`/bench_seed {"record"}`) every 10 s.
//
// Resumable: state in tests/am1/raw/b030-state.json, written every 15 s; the schedule runs on active seconds, so a
// restart continues where the last state left it. Stops at the end of day 30 or when tests/am1/raw/b030-STOP exists.
// Rows go to tests/am1/raw/b030-load.jsonl: each batch, and a summary a minute.
//   node tests/am1/b030/load.mjs
import { createHash, randomBytes } from "node:crypto";
import { existsSync, readFileSync, writeFileSync, renameSync } from "node:fs";
import { deflateSync } from "node:zlib";
import { EDGE, leaderOf, log, member, row, sleep, state as m3, statuses, tokens } from "../hosted/lib.mjs";

const REPO = JSON.parse(readFileSync("tests/am1/raw/b030-repo.json", "utf8"));
const CHAIN = `repo:${REPO.repo}`;
const R = REPO.name;
const STATE = "tests/am1/raw/b030-state.json";
const STOP = "tests/am1/raw/b030-STOP";
const OUT = "tests/am1/raw/b030-load.jsonl";
const DAYS = 30;
const SPEED = Number(process.env.B030_SPEED ?? 18);
const DAY_S = 86_400 / SPEED; // wall seconds per simulated day
const PUSH_PER_S = Number(process.env.B030_PUSHES ?? 1.6);
// Inline pack bytes a simulated day: journey 9's 74.4 GB in 30 days (m0-wp9.md, B0-30) is 2.48. B030_PACK_GB_DAY lowers
// it when the hosted archive alarm cannot drain that at SPEED (tests/am1/results/b0-30.md, "Substitutions").
const PACK_PER_DAY = Number(process.env.B030_PACK_GB_DAY ?? 74.4 / 30) * 1e9;
// B030_PUSHES=0 sends no pushes (tests/am1/results/b0-30.md, "Blocked on", 10).
const PUSH_BYTES = PUSH_PER_S > 0 ? Math.round(PACK_PER_DAY / (PUSH_PER_S * DAY_S)) : 0;
const PER_DAY = { tasks: 6_050, checks: 48_400, provenance: 60_500 };
const BRANCHES = 500;
const BATCH_S = 10;
const ADMIN = 7100003;

// ---- state ----
const st = existsSync(STATE) ? JSON.parse(readFileSync(STATE, "utf8")) : {
  started: new Date().toISOString(), active: 0, pushes: 0, pushFails: 0, next: 0, pushBytes: 0, tips: {},
  tasks: 0, prs: 0, closed: 0, checks: 0, provenance: 0, batches: 0, batchFail: 0, open: [],
  statusId: 1, idBase: 1 << 30,
};
const save = () => { writeFileSync(`${STATE}.tmp`, JSON.stringify(st)); renameSync(`${STATE}.tmp`, STATE); };
const simDay = () => st.active / DAY_S;
const simMs = () => Date.parse("2026-11-10T00:00:00Z") + st.active * SPEED * 1000;

// ---- git ----
const sha1 = (b) => createHash("sha1").update(b).digest();
const obj = (type, body) => sha1(Buffer.concat([Buffer.from(`${type} ${body.length}\0`), body]));
const TYPE = { commit: 1, tree: 2, blob: 3 };
function entryHeader(type, size) {
  const out = [];
  let b = (TYPE[type] << 4) | (size & 15);
  size >>= 4;
  while (size) { out.push(b | 0x80); b = size & 0x7f; size >>= 7; }
  out.push(b);
  return Buffer.from(out);
}
function treeFor(path, blob) {
  const parts = path.split("/");
  const objects = [];
  let child = { mode: "100644", name: parts.pop(), id: blob };
  for (const dir of [...parts.reverse(), null]) {
    const body = Buffer.concat([Buffer.from(`${child.mode} ${child.name}\0`), child.id]);
    const id = obj("tree", body);
    objects.push(["tree", body]);
    if (dir === null) return { root: id, objects };
    child = { mode: "40000", name: dir, id };
  }
}
function onePack(path, bytes, parent, message, at) {
  const { root, objects } = treeFor(path, obj("blob", bytes));
  const who = `b030 <b030@graftyard.invalid> ${at} +0000`;
  const commit = Buffer.from(`tree ${root.toString("hex")}\n${parent ? `parent ${parent}\n` : ""}author ${who}\ncommitter ${who}\n\n${message}\n`);
  const all = [["blob", bytes], ...objects, ["commit", commit]];
  const head = Buffer.alloc(12);
  head.write("PACK"); head.writeUInt32BE(2, 4); head.writeUInt32BE(all.length, 8);
  const body = Buffer.concat([head, ...all.flatMap(([t, b]) => [entryHeader(t, b.length), deflateSync(b, { level: 1 })])]);
  return { pack: Buffer.concat([body, sha1(body)]), commit: obj("commit", commit).toString("hex") };
}
const pkt = (s) => { const b = Buffer.from(s); return Buffer.concat([Buffer.from((b.length + 4).toString(16).padStart(4, "0")), b]); };
const ZERO = "0".repeat(40);
const receiveBody = (ref, old, nu, pack) =>
  Buffer.concat([pkt(`${old ?? ZERO} ${nu} ${ref}\0report-status agent=graftyard-b030\n`), Buffer.from("0000"), pack]);
function pktLines(s) {
  const out = [];
  for (let i = 0; i + 4 <= s.length;) {
    const n = parseInt(s.slice(i, i + 4), 16);
    if (!/^[0-9a-f]{4}$/.test(s.slice(i, i + 4)) || (n > 0 && n < 4)) break;
    if (n === 0) { i += 4; continue; }
    out.push(s.slice(i + 4, i + n));
    i += n;
  }
  return out;
}
function pushResult(text) {
  const lines = pktLines(text);
  const ng = lines.map((l) => l.match(/^ng (\S+) ([^\n]*)/)).find(Boolean);
  if (ng) return /stale|fetch first|non-fast-forward/.test(ng[2]) ? "stale" : `ng ${ng[2].slice(0, 40)}`;
  return lines.some((l) => l.startsWith("ok refs/")) ? "ok" : "error";
}
const auth = (tok) => ({ authorization: `Basic ${Buffer.from(`x:${tok}`).toString("base64")}` });
async function git(path, body, headers, tok = tokens.admin) {
  try {
    const r = await fetch(`${EDGE}/${R}.git/${path}`, { method: "POST", body, headers: { ...auth(tok), "user-agent": "git/2.56.0", ...headers }, signal: AbortSignal.timeout(180_000) });
    return { status: r.status, text: Buffer.from(await r.arrayBuffer()).toString("latin1") };
  } catch (e) { return { status: 0, text: String(e).slice(0, 80) }; }
}
async function tip(ref) {
  const r = await git("git-upload-pack", Buffer.concat([pkt("command=ls-refs\n"), Buffer.from("0001"), pkt(`ref-prefix ${ref}\n`), Buffer.from("0000")]),
    { "content-type": "application/x-git-upload-pack-request", "git-protocol": "version=2" });
  return r.text.match(new RegExp(`([0-9a-f]{40}) ${ref}\\n`))?.[1] ?? null;
}
// The base every blob varies: the same random bytes after a restart, so deltas keep finding it.
const BASE = (() => { const out = []; for (let i = 0; out.length * 32 < PUSH_BYTES + 32; i++) out.push(createHash("sha256").update(`b030-base-${i}`).digest()); return Buffer.concat(out).subarray(0, PUSH_BYTES); })();
let main = null;
const busy = new Set();
async function onePush(k) {
  let i = k % BRANCHES;
  while (busy.has(i)) i = (i + 1) % BRANCHES;
  busy.add(i);
  try { return await pushOn(i, k); } finally { busy.delete(i); }
}
async function pushOn(i, k) {
  const ref = `refs/heads/b030/${i}`;
  const blob = Buffer.from(BASE);
  randomBytes(64).copy(blob, Math.floor(Math.random() * (blob.length - 64)));
  const parent = st.tips[i] ?? main;
  const { pack, commit } = onePack(`b030/${i}/f.bin`, blob, parent, `b030 ${k}`, Math.floor(simMs() / 1000));
  const t0 = Date.now();
  const r = await git("git-receive-pack", receiveBody(ref, st.tips[i] ?? null, commit, pack), { "content-type": "application/x-git-receive-pack-request" });
  pushMs.push(Date.now() - t0);
  const res = r.status === 200 ? pushResult(r.text) : `http ${r.status}`;
  if (res === "ok") { st.tips[i] = commit; st.pushes++; st.pushBytes += pack.length; }
  else if (res === "stale") st.tips[i] = await tip(ref);
  return res;
}
const pushMs = [];

// ---- recorded agent rows ----
const bytes = (s) => [...Buffer.from(s)];
const oid = () => [...randomBytes(20)];
const pad = (n, tag) => (tag + " " + "x".repeat(n)).slice(0, n);
let leader = null;
async function record(events) {
  for (let i = 0; i < 4; i++) {
    try {
      leader ??= await leaderOf(CHAIN);
      const v = (await member(CHAIN, leader, "/bench_seed", { record: events }, 25_000)).v;
      if (v?.recorded) return v.recorded;
      if (v?.refused) throw new Error(`refused ${v.refused}`);
      leader = null;
      await sleep(2_000);
    } catch (e) {
      if (String(e).includes("refused")) throw e;
      leader = null;
      await sleep(3_000);
    }
  }
  return null;
}
/** The batch due now: every row kind brought up to its rate at the current simulated day. */
function batch() {
  const d = simDay();
  const ev = [];
  const due = (have, perDay) => Math.max(0, Math.floor(perDay * d) - have);
  const nTasks = Math.min(40, due(st.tasks, PER_DAY.tasks));
  for (let j = 0; j < nTasks; j++) {
    const t = st.tasks + j;
    const n = 1_000_001 + t;
    const bi = t % BRANCHES;
    const branch = bytes(`refs/heads/b030/${bi}`);
    const tipHex = st.tips[bi] ?? main;
    const head = [...Buffer.from(tipHex, "hex")];
    ev.push({ TaskCreated: { session: `${m3.app}.b030-${t}`, kind: "External", state: "Completed", prefix: null, requester: null,
      app: m3.app, installation: m3.installation, creator: { App: { app: m3.app, installation: m3.installation } }, issue: null,
      snapshot: 1, prompt: null, pr: n, summary: pad(900, `task ${t}`) } });
    ev.push({ PrOpened: { n, head: { repo: REPO.repo, branch }, base: bytes("refs/heads/main"),
      capture: { head_sha: head, base_sha: [...Buffer.from(main, "hex")], state: { Open: { draft: true } }, labels: [],
        issue: { id: st.idBase + t, author: ADMIN, title: `b030 ${t}`, body: pad(1_800, `pr ${t}`), labels: [], assignees: [] }, changes_from: null } } });
    ev.push({ PathSet: { refname: branch, pr: n, head, merge_base: null,
      paths: Array.from({ length: 160 }, (_, p) => pad(100, `src/b030/${t}/${p}`)), planned: [], truncated: false } });
    st.open.push({ n, head, at: st.active + 1200 / SPEED });
  }
  st.tasks += nTasks; st.prs += nTasks;
  while (st.open.length && st.open[0].at <= st.active) {
    const o = st.open.shift();
    ev.push({ PrClosed: { n: o.n, final_head: o.head } });
    st.closed++;
  }
  const nChecks = Math.min(400, due(st.checks, PER_DAY.checks));
  for (let j = 0; j < nChecks; j++) {
    ev.push({ Status: { id: st.statusId++, sha: oid(), context: `ci/${j % 3}`, state: "Success", target_url: null,
      description: pad(220, `check ${st.checks + j}`), creator: ADMIN } });
  }
  st.checks += nChecks;
  const nProv = Math.min(500, due(st.provenance, PER_DAY.provenance));
  for (let j = 0; j < nProv; j++) {
    ev.push({ Provenance: { push: rid26(), list: [{ commit: oid(), refname: bytes(`refs/heads/b030/${(st.provenance + j) % BRANCHES}`),
      sources: [{ Claimed: { key: "GraftyardSession", value: pad(60, `b030-${st.provenance + j}`) } }] }] } });
  }
  st.provenance += nProv;
  return ev;
}
function rid26() {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = BigInt(Date.now()) & 0xffff_ffff_ffffn;
  for (const b of randomBytes(10)) v = (v << 8n) | BigInt(b);
  let out = "";
  for (let i = 25; i >= 0; i--) out += A[Number((v >> BigInt(5 * i)) & 31n)];
  return out;
}

// ---- the schedule ----
const tally = {};
const note = (k) => { tally[k] = (tally[k] ?? 0) + 1; };
// The main tip from the state once read: ls-refs goes through the git cache shard, which may be mid-rehydrate.
main = st.main ?? await tip("refs/heads/main");
st.main = main;
if (!main) throw new Error("no main");
log(`b030 load: ${CHAIN}, speed ${SPEED}, ${PUSH_PER_S} pushes/s of ${PUSH_BYTES} bytes, day ${simDay().toFixed(3)}, pushes ${st.pushes}`);
// The archive alarm promotes through the leader's own site's git cache shard, and only site a's shard has stayed
// warm: shards b and c, cold, took their isolates past memory catching up on 2 GB of log and the archive stopped
// (b0-30.md, "Blocked on", 7). So once a minute, a leader other than HOME_SITE hands leadership back to it when it is
// caught up (member `/transfer`, as tests/am1/runs/transfer.mjs does). B030_HOME=none turns this off.
const HOME_SITE = process.env.B030_HOME === "none" ? null : Number(process.env.B030_HOME ?? 0);
let lastHome = Date.now(), homing = false;
async function keepHome() {
  try {
    const sts = await statuses(CHAIN);
    const l = sts.filter((s) => s.role === "leader").sort((a, b) => b.term - a.term)[0];
    const home = sts.find((s) => s.site === HOME_SITE);
    if (!l || l.site === HOME_SITE || !home || home.applied < l.applied - 2) return;
    const t = await member(CHAIN, l.site, "/transfer", { to: HOME_SITE }, 15_000);
    leader = null;
    note(`home ${l.site}->${HOME_SITE} ${JSON.stringify(t.v?.transfer ?? t.v).slice(0, 40)}`);
  } catch (e) { note(`home error ${String(e).slice(0, 40)}`); }
}
let batching = false, inflight = 0;
let last = Date.now(), lastRow = Date.now(), lastBatch = 0, lastSave = Date.now();
const done = () => simDay() >= DAYS || existsSync(STOP);
while (!done()) {
  const now = Date.now();
  // Active time advances with the wall clock, at most 5 s a tick, so a pause (an orb restart) is not caught up.
  const dt = Math.min(5, (now - last) / 1000);
  st.active += dt;
  st.pushDue = (st.pushDue ?? 0) + PUSH_PER_S * dt;
  last = now;
  while (st.pushDue >= 1 && inflight < 8) {
    st.pushDue -= 1;
    inflight++;
    // Each branch has one push in flight at most, so a push's parent is its branch's last tip.
    onePush(st.next++).then((r) => { note(`push ${r}`); if (r !== "ok") st.pushFails++; }).catch((e) => note(`push error ${String(e).slice(0, 30)}`)).finally(() => inflight--);
  }
  if (now - lastBatch >= BATCH_S * 1000 && !batching) {
    lastBatch = now;
    const ev = batch();
    if (ev.length) {
      batching = true;
      const t0 = Date.now();
      record(ev).then((at) => {
        if (at) { st.batches++; note("batch ok"); row(OUT, { kind: "batch", at, events: ev.length, ms: Date.now() - t0 }); }
        else { st.batchFail++; note("batch lost"); }
      }).catch((e) => { st.batchFail++; note(`batch ${String(e).slice(0, 60)}`); }).finally(() => { batching = false; });
    }
  }
  if (now - lastSave > 15_000) { save(); lastSave = now; }
  if (HOME_SITE !== null && now - lastHome > 60_000 && !homing) {
    lastHome = now;
    homing = true;
    keepHome().finally(() => { homing = false; });
  }
  if (now - lastRow > 60_000) {
    lastRow = now;
    row(OUT, { kind: "minute", at: new Date().toISOString(), day: +simDay().toFixed(4), pushes: st.pushes, pushBytes: st.pushBytes,
      tasks: st.tasks, closed: st.closed, checks: st.checks, provenance: st.provenance, batches: st.batches, batchFail: st.batchFail, inflight,
      pushMs: pushMs.length ? { n: pushMs.length, p50: pushMs.sort((a, b) => a - b)[pushMs.length >> 1], max: pushMs.at(-1) } : null, tally });
    for (const k of Object.keys(tally)) delete tally[k];
    pushMs.length = 0;
  }
  await sleep(250);
}
while (inflight || batching) await sleep(500);
save();
log(`b030 load ended at day ${simDay().toFixed(3)}: ${JSON.stringify({ pushes: st.pushes, tasks: st.tasks, checks: st.checks, provenance: st.provenance })}`);
writeFileSync("tests/am1/raw/b030-load.done", new Date().toISOString() + "\n");
