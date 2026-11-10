// B0-30's replay: 30 simulated days of journey 9 on m3org/b0-30 in about 40 h (SPEED 18), at journey 9's bytes per
// day (tests/m0/results/m0-wp9.md, B0-30), with these substitutions (tests/am1/results/b0-30.md, "Substitutions"):
//
// - **Log bytes.** M0 replayed 1.815 M agent pushes of 10 to 70 KB (74.4 GB uncapped). Here the same 2.48 GB a
//   simulated day goes as real git pushes through graft-m1's edge, PUSH_BYTES each, at about 0.5 a second, by
//   m3-admin (Human class, so packs stay inline as M0's uncapped volume). Each pack is one commit of one blob: a
//   fixed 1 MB base with 64 random bytes replaced, so the git cache's repacks and the archive alarm's promotions
//   delta them and the shard's disk does not take 74 GB.
// - **Agent rows.** M0's rows per simulated day (6,050 tasks with a 2 KB PR row and a 1 KB task row each, 48,400
//   300-byte check rows, 60,500 150-byte provenance rows, a 16 KB `PathSet` per open PR deleted 20 simulated minutes
//   later) are recorded by the leader in batched entries (`/bench_seed {"record"}`), not as 10 M separate commands.
// - **Live commands.** A sample of agent commands through the edge (task summary edits, lease acquire, renew and
//   release, `addComment`) at LIVE_PER_S, so the edge, admission, and decide paths run beside the batches.
//
// Resumable: state in tests/am1/raw/b030-state.json, written every 15 s; the schedule runs on active seconds, so a
// restart continues where the last state left it. Stops at the end of day 30 or when tests/am1/raw/b030-STOP exists.
// Rows go to tests/am1/raw/b030-load.jsonl, one a minute.
//   node tests/am1/b030/load.mjs
import { createHash, randomBytes } from "node:crypto";
import { existsSync, readFileSync, writeFileSync, renameSync } from "node:fs";
import { deflateSync } from "node:zlib";
import { EDGE, api, ghs, leaderOf, log, member, row, sleep, state as m3, tokens } from "../hosted/lib.mjs";

const REPO = JSON.parse(readFileSync("tests/am1/raw/b030-repo.json", "utf8"));
const CHAIN = `repo:${REPO.repo}`;
const R = REPO.name;
const STATE = "tests/am1/raw/b030-state.json";
const STOP = "tests/am1/raw/b030-STOP";
const OUT = "tests/am1/raw/b030-load.jsonl";
const DAYS = 30;
const SPEED = Number(process.env.B030_SPEED ?? 18);
const DAY_S = 86_400 / SPEED; // wall seconds per simulated day
const PUSH_BYTES = 1_000_000;
const PER_DAY = { packBytes: 74.4e9 / 30, tasks: 6_050, checks: 48_400, provenance: 60_500 };
const BRANCHES = 500;
const LIVE_PER_S = Number(process.env.B030_LIVE ?? 0.5);
const BATCH_S = 10;
const ADMIN = 7100003;

// ---- state ----
const st = existsSync(STATE) ? JSON.parse(readFileSync(STATE, "utf8")) : {
  started: new Date().toISOString(), active: 0, pushes: 0, pushBytes: 0, pushFail: {}, tips: {},
  tasks: 0, prs: 0, closed: 0, checks: 0, provenance: 0, batches: 0, batchFail: 0, open: [], live: {}, liveTasks: [],
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
async function git(path, body, headers) {
  try {
    const r = await fetch(`${EDGE}/${R}.git/${path}`, { method: "POST", body, headers: { ...auth(tokens.admin), "user-agent": "git/2.56.0", ...headers }, signal: AbortSignal.timeout(180_000) });
    return { status: r.status, text: Buffer.from(await r.arrayBuffer()).toString("latin1") };
  } catch (e) { return { status: 0, text: String(e).slice(0, 80) }; }
}
async function tip(ref) {
  const r = await git("git-upload-pack", Buffer.concat([pkt("command=ls-refs\n"), Buffer.from("0001"), pkt(`ref-prefix ${ref}\n`), Buffer.from("0000")]),
    { "content-type": "application/x-git-upload-pack-request", "git-protocol": "version=2" });
  return r.text.match(new RegExp(`([0-9a-f]{40}) ${ref}\\n`))?.[1] ?? null;
}
// The 1 MB base every blob varies: the same bytes after a restart, so deltas keep finding it.
const BASE = (() => { const out = []; for (let i = 0; out.length * 32 < PUSH_BYTES; i++) out.push(createHash("sha256").update(`b030-base-${i}`).digest()); return Buffer.concat(out).subarray(0, PUSH_BYTES); })();
let main = null;
async function onePush(k) {
  const i = k % BRANCHES;
  const ref = `refs/heads/claude/b030-${i}`;
  const blob = Buffer.from(BASE);
  randomBytes(64).copy(blob, (k * 7919) % (PUSH_BYTES - 64));
  const parent = st.tips[i] ?? main;
  const { pack, commit } = onePack(`b030/${i}/f.bin`, blob, parent, `b030 ${k}`, Math.floor(simMs() / 1000));
  const r = await git("git-receive-pack", receiveBody(ref, st.tips[i] ?? null, commit, pack), { "content-type": "application/x-git-receive-pack-request" });
  const res = r.status === 200 ? pushResult(r.text) : `http ${r.status}`;
  if (res === "ok") { st.tips[i] = commit; st.pushes++; st.pushBytes += pack.length; }
  else if (res === "stale") st.tips[i] = await tip(ref);
  return res;
}

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
    const branch = bytes(`refs/heads/claude/b030-pr-${t}`);
    const head = oid();
    ev.push({ TaskCreated: { session: `${m3.app}.b030-${t}`, kind: "External", state: "Completed", prefix: null, requester: null,
      app: m3.app, installation: m3.installation, creator: { App: { app: m3.app, installation: m3.installation } }, issue: null,
      snapshot: 1, prompt: null, pr: n, summary: pad(900, `task ${t}`) } });
    ev.push({ PrOpened: { n, head: { repo: REPO.repo, branch }, base: bytes("refs/heads/main"),
      capture: { head_sha: head, base_sha: oid(), state: { Open: { draft: true } }, labels: [],
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
    ev.push({ Provenance: { push: rid26(), list: [{ commit: oid(), refname: bytes(`refs/heads/claude/b030-${(st.provenance + j) % BRANCHES}`),
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

// ---- live commands through the edge ----
let app = null, appAt = 0;
async function appToken() {
  if (!app || Date.now() - appAt > 40 * 60_000) {
    const r = await ghs({});
    if (r.status !== 201) throw new Error(`ghs ${r.status}`);
    app = r.body.token; appAt = Date.now();
  }
  return app;
}
const V1 = `/api/graftyard/v1/repos/${R}`;
let target = null;
async function liveOne(k) {
  const tok = await appToken();
  const kind = ["task", "lease", "renew", "comment"][k % 4];
  if (st.liveTasks.length < 20) {
    const r = await api("POST", `${V1}/tasks`, tok, { kind: "external", session: `b030-live-${st.liveTasks.length}-${Date.now().toString(36)}`, summary: "b030 live" });
    if (r.status === 201) st.liveTasks.push({ id: r.body.id, lease: null });
    return `task:create ${r.status}`;
  }
  const t = st.liveTasks[k % st.liveTasks.length];
  const h = { "x-graftyard-agent-session": t.id };
  if (kind === "task") return `task ${(await api("PATCH", `${V1}/tasks/${t.id}`, tok, { summary: pad(400, `live ${k}`) })).status}`;
  if (kind === "lease") {
    if (t.lease) { const r = await api("DELETE", `${V1}/leases/${t.lease}`, tok, undefined, h); t.lease = null; return `lease:release ${r.status}`; }
    const r = await api("POST", `${V1}/leases`, tok, { paths: [`b030/live/${k % 997}/**`], mode: "shared", ttl_seconds: 600 }, h);
    if (r.status === 201) t.lease = r.body?.lease?.id ?? null;
    return `lease:acquire ${r.status}`;
  }
  if (kind === "renew") return t.lease ? `lease:renew ${(await api("POST", `${V1}/leases/${t.lease}/renew`, tok, { ttl_seconds: 600 }, h)).status}` : "lease:none";
  if (!target) {
    const q = await api("POST", "/api/graphql", tokens.admin, { query: `query { repository(owner: "m3org", name: "b0-30") { id issues(first: 1) { nodes { id number } } } }` });
    target = q.body?.data?.repository?.issues?.nodes?.[0];
    if (!target) {
      const c = await api("POST", "/api/graphql", tokens.admin, { query: "mutation($i: CreateIssueInput!) { createIssue(input: $i) { issue { id number } } }", variables: { i: { repositoryId: q.body?.data?.repository?.id, title: "b030 live target" } } });
      target = c.body?.data?.createIssue?.issue;
    }
    if (!target) return "comment:no target";
  }
  const r = await api("POST", "/api/graphql", tok, { query: "mutation($i: AddCommentInput!) { addComment(input: $i) { subject { id } } }", variables: { i: { subjectId: target.id, body: pad(300, `live ${k}`) } } });
  return r.body?.data?.addComment ? "comment ok" : `comment ${String(r.body?.errors?.[0]?.message ?? r.status).slice(0, 40)}`;
}

// ---- the schedule ----
const tally = {};
const note = (k) => { tally[k] = (tally[k] ?? 0) + 1; };
main = await tip("refs/heads/main");
if (!main) throw new Error("no main");
// The fleet's App classes as Agent, as a1.mjs flags it.
const flag = await api("PUT", `/api/graftyard/v1/agents/apps/${m3.app}`, tokens.admin, { kind: "third_party", confine: "refs/heads/claude/**" });
log(`b030 load: ${CHAIN}, speed ${SPEED}, day ${simDay().toFixed(3)}, pushes ${st.pushes}, flag ${flag.status}`);
let inflight = 0, liveInflight = 0, liveK = Object.values(st.live).reduce((a, b) => a + b, 0);
let last = Date.now(), lastRow = Date.now(), lastBatch = 0, lastSave = Date.now();
const done = () => simDay() >= DAYS || existsSync(STOP);
while (!done()) {
  const now = Date.now();
  // Active time advances with the wall clock, at most 5 s a tick, so a pause (an orb restart) is not caught up.
  st.active += Math.min(5, (now - last) / 1000);
  last = now;
  const wantPushes = Math.floor((PER_DAY.packBytes * simDay()) / PUSH_BYTES);
  while (st.pushes + inflight < wantPushes && inflight < 4) {
    inflight++;
    onePush(st.pushes + inflight + Math.floor(Math.random() * 1e6)).then((r) => note(`push ${r}`)).catch((e) => note(`push error ${String(e).slice(0, 30)}`)).finally(() => inflight--);
  }
  if (now - lastBatch >= BATCH_S * 1000) {
    lastBatch = now;
    const ev = batch();
    if (ev.length) {
      const t0 = Date.now();
      try {
        const at = await record(ev);
        if (at) { st.batches++; note("batch ok"); row(OUT, { kind: "batch", at, events: ev.length, ms: Date.now() - t0 }); }
        else { st.batchFail++; note("batch lost"); }
      } catch (e) { st.batchFail++; note(`batch ${String(e).slice(0, 60)}`); }
    }
  }
  const wantLive = Math.floor(st.active * LIVE_PER_S);
  while (liveK < wantLive && liveInflight < 4) {
    liveInflight++;
    const k = liveK++;
    liveOne(k).then((r) => { note(r); st.live[r] = (st.live[r] ?? 0) + 1; }).catch((e) => note(`live error ${String(e).slice(0, 30)}`)).finally(() => liveInflight--);
  }
  if (now - lastSave > 15_000) { save(); lastSave = now; }
  if (now - lastRow > 60_000) {
    lastRow = now;
    row(OUT, { kind: "minute", at: new Date().toISOString(), day: +simDay().toFixed(4), pushes: st.pushes, pushBytes: st.pushBytes,
      tasks: st.tasks, closed: st.closed, checks: st.checks, provenance: st.provenance, batches: st.batches, batchFail: st.batchFail, tally });
    for (const k of Object.keys(tally)) delete tally[k];
  }
  await sleep(250);
}
while (inflight || liveInflight) await sleep(500);
save();
log(`b030 load ended at day ${simDay().toFixed(3)}: ${JSON.stringify({ pushes: st.pushes, tasks: st.tasks, checks: st.checks, provenance: st.provenance })}`);
writeFileSync("tests/am1/raw/b030-load.done", new Date().toISOString() + "\n");
