// Journey 9 / A1 replay against an install's edge (`build/BUILD-PLAN.md`, M0-WP9 rows B0-29 and B0-31 to B0-35, and
// L0-12's A1 replay and 500-poller line), sized by `journey9` below from the rows and `scripts/cost_agents.py --a1`.
// The orb sends every request: no load Workers, no member shortcuts for writes.
//
//   - Agents push their own ref `refs/heads/claude/a1-<run>-<i>` every `pushEvery` s with an in-process pack (one blob
//     under `a1/<group>/<i>/`, its tree, a commit on `main`), straight to `git-receive-pack` under a token of the
//     confined vendor App (m3-ci, flagged `third_party`, `confine refs/heads/claude/**`), so each push is Agent class
//     and leaves a `PathSet` the feed carries.
//   - Agent side commands: task summary edits, comments (GraphQL `addComment`), labels (REST `issues/{n}/labels`),
//     draft PR opens, lease acquires, renewals, and releases, at the A1 mix's rates where the edge serves the route.
//   - Humans: pushes and comments under the seeded admin's PAT; merges: admin PRs merged through `PUT pulls/{n}/merge`.
//   - Reads: v2 `ls-refs` (ref reads), REST and GraphQL reads (API reads), and clone time to first byte.
//   - Feed: `pollers` long-polls straight to the group's followers and learners (`/member` `/feed`, as the edge sends
//     them, each with its own token and IP hash, because one orb IP is held to FEED_POLLS' 50 per member), each with
//     `paths` of one of 50 agent groups; each event's latency is the poll's return minus the push's acknowledgment.
//   - Faults: `--kill-every S` sends one push in S with `x-graft-kill: after_quorum` (L0-12's leader kills);
//     `--kill-shard-at S` tears down the home shard once (B0-34's warm-site kill).
//
// Rows: b0-29, b0-31 (every agent push 1 MiB, leader on apac: run on a group whose leader was moved there), b0-32 (the
// burst), b0-33 (5,000 agents plus one fast user token), b0-34 (journey 9 plus clone TTFB and the shard kill), b0-35
// (500 open PRs whose `mergeable` 500 clients poll every 5 s while the base moves 10 times a minute), l0-12 (a lease
// per agent, leader kills, 500 pollers).
//
//   node tests/am1/rest/a1.mjs --dry <row> [--agents N] [--seconds S]   # the plan, offline; checks the rates
//   node tests/am1/rest/a1.mjs --selftest                               # the pack builder against git, offline
//   node tests/am1/rest/a1.mjs <row> --repo NAME --chain repo:ID [--seconds 600] [--agents 500] [--kill-every 60]
//     [--push-every S]   # each agent's push interval, for a scaled run (default the row's 120 s)
//     [--as-admin]       # every request under the admin's PAT, for installs that cannot mint App tokens
//     [--rest-polls F]   # B0-35: this share of the `mergeable` polls is REST `GET pulls/{n}` (default 0, all GraphQL)
//     [--first-for MS]   # B0-35, test installs: the leader gives each `PrSync` first attempt MS before the queue re-sends it
//
// B0-35 pushes to its open PRs' head branches, so each push asks the network's shard for a test merge, adds `mergeable`
// to its list reads, and writes `testMerges`: each site's shard budget over the window (`/_bed/shard_status`, the
// shard's `graftyard_git::prsync::BudgetStats`), the vCPU used against the budget, list-field computes, and the
// leader's `/stats` `testMerges` (asked and shed by its first attempts). Its `mergeable` polls are single-PR GraphQL
// reads under the App's token, which the edge turns into GET merges (`getAsked`, `getComputes`); its list reads under
// the admin's PAT are counted (`listAsked`). `overrun` is whether any shard's bucket went more than one merge below zero.
//
// Long runs go in an orb service: amp orb service start a1-<row> --command "node tests/am1/rest/a1.mjs <row> ... >
// tests/am1/raw/a1-<row>.log 2>&1; sleep infinity". Environment as `tests/am1/hosted/lib.mjs` (GRAFT_EDGE,
// GRAFT_TOOLS_EDGE, M3_STATE, BED_TOKEN_FILE); every token comes from those files or a mint, goes only in an
// `authorization` header, and every log line passes `scan`. Rows go to tests/am1/raw/a1-<row>-<run>.jsonl.

import { createHash, randomBytes } from "node:crypto";
import { execFileSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { deflateSync } from "node:zlib";
import { parseArgs } from "node:util";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, strict: true, options: {
  dry: { type: "boolean" }, selftest: { type: "boolean" }, seconds: { type: "string" }, agents: { type: "string" },
  repo: { type: "string" }, chain: { type: "string" }, "kill-every": { type: "string" }, "kill-shard-at": { type: "string" },
  run: { type: "string" }, "api-reads": { type: "string" }, "ref-reads": { type: "string" }, pollers: { type: "string" },
  "push-every": { type: "string" }, "as-admin": { type: "boolean" }, "rest-polls": { type: "string" },
  "first-for": { type: "string" } } });

// ---- the plan, from the rows ----

/** Journey 9 at `agents` (B0-29; `cost_agents.py --a1`): pushes every 120 s (40 KB mean, 5% at 1 MiB), the A1 agent
 *  side-command mix per 500 agents, 1.25 human and 1.0 bot commands/s, 10 merges/min, 33 ref reads/s, 200 API
 *  reads/s, 500 pollers. Rates are whole-fleet per second. */
export function journey9(agents) {
  const f = agents / 500;
  return {
    agents, pushEvery: 120, packMin: 10_000, packMax: 70_000, bigShare: 0.05, bigBytes: 1 << 20,
    // cost_agents.py --a1, agent class per 500 agents, the parts the edge serves a route for. PrSync and test-merge
    // records (4.17/s each) are the host's own commands after each push; RequiredCheck and check runs (3.33/s) have
    // no edge route and are not offered (`unserved`).
    // `standIn` is task summary edits at the unserved rate, so the group sees the row's command rate; they are
    // agent-class commands of one small entry each, as a check run is.
    side: { taskEdit: 1.67 * f, comment: 1.88 * f, label: 1.87 * f, draftPr: 0.42 * f, leaseAcquire: 0.42 * f,
      leaseRelease: 0.41 * f, leaseRenew: 2.5 * f, sessionPush: 0.21 * f, standIn: 3.33 * f },
    hostMade: 8.34 * f, unserved: 3.33 * f,
    human: 1.25, bot: 1.0, mergesPerMin: 10, refReads: 33, apiReads: 200, clones: 0, pollers: 500,
  };
}

export function planFor(row, { agents, seconds, apiReads, refReads }) {
  const base = { row, seconds, ...journey9(agents ?? 500), ...(apiReads !== undefined ? { apiReads } : {}), ...(refReads !== undefined ? { refReads } : {}) };
  switch (row) {
    case "b0-29": return base;
    case "b0-31": return { ...base, packFixed: 1 << 20 };
    case "b0-32": return { ...base, burst: { at: Math.min(60, seconds / 3), pushes: 500, bytes: 256 << 10, merges: 5 } };
    case "b0-33": return { ...journey9(agents ?? 5000), row, seconds, fastUser: 100, pollers: 0, apiReads: 0 };
    case "b0-34": return { ...base, clones: 1 };
    // An open PR and a poller per agent: 500 each at the row's 500 agents, fewer in a scaled run.
    case "b0-35": return { ...base, openPrs: base.agents, prPollers: base.agents, prPollEvery: 5 };
    case "l0-12": return { ...base, leases: true, killEvery: 60 };
    default: throw new Error(`unknown row ${row}`);
  }
}

/** What the plan offers, per class, against what the rows name. */
export function offered(p) {
  const pushes = p.agents / p.pushEvery;
  const side = Object.values(p.side).reduce((a, b) => a + b, 0);
  return {
    agentPushesPerS: +pushes.toFixed(2), agentSidePerS: +side.toFixed(2), agentClassPerS: +(pushes + side).toFixed(2),
    hostMadePerS: +p.hostMade.toFixed(2), unservedPerS: +p.unserved.toFixed(2),
    groupCommandsPerS: +(pushes + side + p.hostMade + p.human + p.bot + p.mergesPerMin / 60 + (p.fastUser ?? 0) / 60).toFixed(1),
    agentClassWithHostPerS: +(pushes + side + p.hostMade).toFixed(1),
    meanPackBytes: p.packFixed ?? Math.round((1 - p.bigShare) * (p.packMin + p.packMax) / 2 + p.bigShare * p.bigBytes),
    feedPollers: p.pollers, refReadsPerS: p.refReads, apiReadsPerS: p.apiReads, mergesPerMin: p.mergesPerMin,
  };
}

const FIELDS = ["spentMs", "wallMs", "computed", "shed", "getAsked", "getComputes", "listAsked", "listComputes"];
/** The window's counts per site: `after` less `before`, or `after` alone if the shard object restarted (`since`). */
export function budgetWindow(before, after, seconds) {
  const sites = after.map((a, site) => {
    if (!a) return null;
    const b = before[site];
    const restarted = !b || b.since !== a.since;
    // `lowMs` and `maxMs` are the bucket's lowest and the largest charge since `since`, so they cover the window only
    // on a shard that restarted for it; otherwise they are an upper bound on the overrun over the shard's life.
    return { site, vcpu: a.vcpu, restarted, lowMs: a.lowMs ?? 0, maxMs: a.maxMs ?? 0, inFlightMax: a.inFlightMax ?? 0,
      ...Object.fromEntries(FIELDS.map((f) => [f, (a[f] ?? 0) - (restarted ? 0 : b[f] ?? 0)])) };
  }).filter(Boolean);
  const sum = (f) => sites.reduce((n, x) => n + x[f], 0);
  const vcpuUsed = +(sum("spentMs") / (seconds * 1000)).toFixed(4);
  const budgetVcpu = sites.length ? Math.max(...sites.map((x) => x.vcpu)) : null;
  // Overrun beyond one merge: the bucket went lower than the largest single charge (or the 50 ms estimate) below zero.
  const overrun = sites.some((x) => x.lowMs < -Math.max(x.maxMs, 50) - 0.5);
  return { sites, vcpuUsed, budgetVcpu, withinBudget: budgetVcpu !== null && vcpuUsed <= budgetVcpu, overrun,
    inFlightMax: Math.max(0, ...sites.map((x) => x.inFlightMax)),
    computed: sum("computed"), shed: sum("shed"), getAsked: sum("getAsked"), getComputes: sum("getComputes"),
    listAsked: sum("listAsked"), listComputes: sum("listComputes"), cpuToWall: sum("wallMs") ? +(sum("spentMs") / sum("wallMs")).toFixed(3) : null };
}

// ---- packs: one blob, its tree, and a commit, as git writes them ----

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
/** A tree of nested directories down to one file at `path`. */
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
/** The pack and the commit id of one commit on `parent` (hex, or null) holding `bytes` at `path`. */
export function onePack(path, bytes, parent, message, at = 1_700_000_000) {
  const blobId = obj("blob", bytes);
  const { root, objects } = treeFor(path, blobId);
  const who = `a1 <a1@graftyard.invalid> ${at} +0000`;
  const commit = Buffer.from(`tree ${root.toString("hex")}\n${parent ? `parent ${parent}\n` : ""}author ${who}\ncommitter ${who}\n\n${message}\n`);
  const all = [["blob", bytes], ...objects, ["commit", commit]];
  const head = Buffer.alloc(12);
  head.write("PACK"); head.writeUInt32BE(2, 4); head.writeUInt32BE(all.length, 8);
  const body = Buffer.concat([head, ...all.flatMap(([t, b]) => [entryHeader(t, b.length), deflateSync(b, { level: 1 })])]);
  return { pack: Buffer.concat([body, sha1(body)]), commit: obj("commit", commit).toString("hex") };
}
const pkt = (s) => { const b = Buffer.from(s); return Buffer.concat([Buffer.from((b.length + 4).toString(16).padStart(4, "0")), b]); };
const ZERO = "0".repeat(40);
/** The receive-pack request body for one ref update, with report-status and no side-band. */
export const receiveBody = (ref, old, nu, pack) =>
  Buffer.concat([pkt(`${old ?? ZERO} ${nu} ${ref}\0report-status agent=graftyard-a1\n`), Buffer.from("0000"), pack]);
/** The payloads of a pkt-line stream (flushes dropped); stops at the first malformed length. */
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
/** `ok`, or the `ng` reason, from a report-status reply, side-band framed or not. A status line is read by its
 * pkt-line length, not by the text before it, since a length can end in a hex letter (`002cok refs/...`). */
export function pushResult(text) {
  const raw = pktLines(text);
  const band1 = raw.filter((l) => l[0] === "\x01").map((l) => l.slice(1)).join("");
  const lines = raw.some((l) => l[0] === "\x01") ? pktLines(band1) : raw.filter((l) => l[0] !== "\x02" && l[0] !== "\x03");
  const ng = lines.map((l) => l.match(/^ng (\S+) ([^\n]*)/)).find(Boolean);
  if (ng) return ng[2].includes("admission full") ? "full" : /stale|fetch first|non-fast-forward/.test(ng[2]) ? "stale" : "refused";
  return lines.some((l) => l.startsWith("ok refs/")) ? "ok" : "error";
}

if (opt.selftest) {
  const dir = mkdtempSync(join(tmpdir(), "a1-selftest-"));
  try {
    execFileSync("git", ["init", "-q", "--bare", dir]);
    const a = onePack("a1/3/7/f.txt", randomBytes(40_000), null, "first");
    execFileSync("git", ["index-pack", "--stdin", "--strict", "--fix-thin"], { cwd: dir, input: a.pack });
    const b = onePack("a1/3/7/f.txt", Buffer.from("second\n"), a.commit, "second");
    execFileSync("git", ["index-pack", "--stdin", "--strict"], { cwd: dir, input: b.pack });
    const log = execFileSync("git", ["log", "--format=%s", b.commit], { cwd: dir }).toString().trim();
    const ok = log === "second\nfirst" && execFileSync("git", ["cat-file", "-p", `${b.commit}:a1/3/7/f.txt`], { cwd: dir }).toString() === "second\n";
    const s = (...ls) => Buffer.concat([...ls.map(pkt), Buffer.from("0000")]).toString("latin1");
    const r = pushResult(s("unpack ok\n", "ok refs/heads/claude/a1-x-1\n"));
    // A length ending in a hex letter: `002cok refs/...` (am1-runs' and fanout's B0-35 setup read these as errors).
    const hex = pushResult(s("unpack ok\n", "ok refs/heads/claude/a1-mwa4unw0-pr10\n"));
    const n = pushResult(s("unpack ok\n", "ng refs/heads/x agent admission full, retry after 2s\n"));
    const band = pushResult(s("\x02remote: hi\n", `\x01${s("unpack ok\n", "ok refs/heads/claude/a1-mwa4unw0-pr123\n")}`));
    // B0-35's window: a delta on a shard that kept its budget, the whole count on one that restarted, none on a site
    // whose shard never merged.
    const bs = (since, spentMs, computed, shed, x = {}) => ({ since, vcpu: 0.25, spentMs, wallMs: spentMs * 2, computed, shed,
      getAsked: 0, getComputes: 0, listAsked: 0, listComputes: 0, lowMs: -40, maxMs: 30, ...x });
    const w = budgetWindow([bs(1, 100, 2, 0, { getAsked: 5, listAsked: 3 }), bs(5, 900, 9, 9), null],
      [bs(1, 6100, 52, 3, { getAsked: 25, getComputes: 4, listAsked: 10 }), bs(7, 400, 4, 1, { inFlightMax: 3 }), null], 60);
    // An older shard (no `getAsked`) reads as 0, and a bucket 400 ms below zero with 100 ms merges is an overrun.
    const old = budgetWindow([null], [{ since: 1, vcpu: 0.25, spentMs: 60, computed: 1, shed: 0, listAsked: 0, listComputes: 0 }], 60);
    const over = budgetWindow([null], [bs(1, 600, 6, 0, { lowMs: -400, maxMs: 100 })], 60);
    const win = w.sites.length === 2 && w.computed === 54 && w.shed === 4 && w.vcpuUsed === 0.1067 && w.withinBudget && w.sites[1].restarted
      && w.getAsked === 20 && w.getComputes === 4 && w.listAsked === 7 && w.cpuToWall === 0.5 && !w.overrun
      && old.getAsked === 0 && !old.overrun && over.overrun && w.inFlightMax === 3 && old.inFlightMax === 0
      // A shard object that restarted after the load keeps the run's last sample; one still up reads now.
      && latestBudget([bs(9, 0, 0, 0), null, bs(1, 700, 7, 0)], [bs(1, 600, 6, 0), null, bs(1, 600, 6, 0)])
        .map((b) => b?.computed ?? null).join() === "6,,7";
    console.log(JSON.stringify({ packs: ok, ok: r, hex, full: n, band, budgetWindow: win }));
    if (!ok || r !== "ok" || hex !== "ok" || n !== "full" || band !== "ok" || !win) process.exit(1);
  } finally { rmSync(dir, { recursive: true, force: true }); }
  process.exit(0);
}

const row = positionals[0];
const seconds = Number(opt.seconds ?? 600);
// `--api-reads` and `--ref-reads` override the row's read rates (a diagnostic run; the gate runs at the row's).
const plan = planFor(row, { agents: opt.agents ? Number(opt.agents) : undefined, seconds,
  apiReads: opt["api-reads"] !== undefined ? Number(opt["api-reads"]) : undefined, refReads: opt["ref-reads"] !== undefined ? Number(opt["ref-reads"]) : undefined });
if (opt["kill-every"]) plan.killEvery = Number(opt["kill-every"]);
if (opt.pollers !== undefined) plan.pollers = Number(opt.pollers);
if (opt["push-every"] !== undefined) plan.pushEvery = Number(opt["push-every"]);
const restPolls = Math.min(1, Math.max(0, Number(opt["rest-polls"] ?? 0)));

if (opt.dry) {
  const o = offered(plan);
  // The rows' rates, which the plan must meet: B0-29's 27.6 commands/s (the A1 table's), within the unserved part.
  const checks = {
    pushes: row === "b0-33" || Math.abs(o.agentPushesPerS - plan.agents / 120) < 0.01,
    // B0-29: the A1 table's 27.6 commands/s; B0-33: its 252 agent-class commands/s offered at 5,000 agents.
    groupRate: row === "b0-33" ? Math.abs(o.agentClassWithHostPerS - 252) < 3 : Math.abs(o.groupCommandsPerS - 27.6) < 0.6,
    pollers: row === "b0-33" ? o.feedPollers === 0 : o.feedPollers === 500,
    meanPack: row === "b0-31" ? o.meanPackBytes === 1 << 20 : o.meanPackBytes > 30_000 && o.meanPackBytes < 100_000,
  };
  console.log(JSON.stringify({ plan, offered: o, checks }, null, 2));
  process.exit(Object.values(checks).every(Boolean) ? 0 : 1);
}

// ---- the live run ----

if (!opt.repo || !opt.chain) throw new Error("--repo NAME and --chain repo:ID are required for a live run");
const { api, bedGet, ghs, log, member, row: writeRow, sleep, state, statuses, tokens } = await import("../hosted/lib.mjs");
const RUN = opt.run ?? Date.now().toString(36);
const R = `m3org/${opt.repo}`;
const CHAIN = opt.chain;
const EDGE = process.env.GRAFT_EDGE;
const RAW = new URL("../raw/", import.meta.url).pathname;
mkdirSync(RAW, { recursive: true });
const OUT = `${RAW}a1-${row}-${RUN}.jsonl`;
// Set again once setup (tasks, leases, open PRs) is done, so setup never eats the load window.
let t0 = Date.now() + 15_000;
let end = t0 + seconds * 1000;
const lat = {};
const codes = {};
const note = (k, ms, code) => { (lat[k] ??= []).push(ms); const c = (codes[k] ??= {}); c[code] = (c[code] ?? 0) + 1; };
const pct = (xs, p) => { const s = [...(xs ?? [])].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : null; };

// Tokens: m3-ci installation tokens, refreshed every 40 min; the admin's PAT for humans and merges.
let app = null;
const admin = tokens.admin;
// `--as-admin`: pushes are Human class, so the run measures the test-merge budget, not B0-35's admission.
const refreshApp = async () => {
  if (opt["as-admin"]) { app = admin; return; }
  const r = await ghs({}); if (r.status !== 201) throw new Error(`ghs ${r.status}`); app = r.body.token;
};
// The fleet's App is the confined vendor App, as B0-36 ran it: every push and command it sends is Agent class.
if (!opt["as-admin"]) {
  const flag = await api("PUT", `/api/graftyard/v1/agents/apps/${state.app}`, admin, { kind: "third_party", confine: "refs/heads/claude/**" });
  if (flag.status !== 200) throw new Error(`flag m3-ci: ${flag.status}`);
}
await refreshApp();
const auth = (tok) => ({ authorization: `Basic ${Buffer.from(`x:${tok}`).toString("base64")}` });

async function git(path, tok, body, headers = {}) {
  const t = Date.now();
  try {
    const r = await fetch(`${EDGE}/${R}.git/${path}`, { method: "POST", body, headers: { ...auth(tok), "user-agent": "git/2.56.0", ...headers }, signal: AbortSignal.timeout(120_000) });
    return { status: r.status, text: Buffer.from(await r.arrayBuffer()).toString("latin1"), ms: Date.now() - t, headers: r.headers };
  } catch (e) { return { status: 0, text: String(e).slice(0, 80), ms: Date.now() - t }; }
}
const lsRefs = (tok, prefix) => git("git-upload-pack", tok, Buffer.concat([pkt("command=ls-refs\n"), Buffer.from("0001"), pkt(`ref-prefix ${prefix}\n`), Buffer.from("0000")]),
  { "content-type": "application/x-git-upload-pack-request", "git-protocol": "version=2" });
const tip = async (ref) => (await lsRefs(admin, ref)).text.match(new RegExp(`([0-9a-f]{40}) ${ref}\\n`))?.[1] ?? null;
let main = await tip("refs/heads/main");
if (!main) throw new Error("the repository has no main");

/** One push of `bytes` to `ref` from `from` (old tip or null); returns [result, ms, new tip]. */
const pushed = new Map(); // commit id -> ack time, for feed latency
async function push(tok, ref, from, path, bytes, extra = {}) {
  const { pack, commit } = onePack(path, bytes, from ?? main, `a1 ${ref}`, Math.floor(Date.now() / 1000));
  const r = await git("git-receive-pack", tok, receiveBody(ref, from, commit, pack), { "content-type": "application/x-git-receive-pack-request", ...extra });
  // A refusal keeps the first words of its body, so a 503's cause is counted (no secret is in a server's reply,
  // and every row passes `scan`).
  const res = r.status === 200 ? pushResult(r.text) : `http ${r.status} ${String(r.text ?? "").replace(/[0-9a-f]{12,}/g, "H").replace(/\d+/g, "N").slice(0, 160).trim()}`;
  if (res === "ok") pushed.set(commit, Date.now());
  return [res, r.ms, res === "ok" ? commit : from];
}
const size = () => plan.packFixed ?? (Math.random() < plan.bigShare ? plan.bigBytes : plan.packMin + Math.floor(Math.random() * (plan.packMax - plan.packMin)));

// Per-agent state: its ref, tip, group, and (L0-12) task and lease.
const agents = Array.from({ length: plan.agents }, (_, i) => ({ i, ref: `refs/heads/claude/a1-${RUN}-${i}`, tip: null, group: i % 50, task: null, lease: null }));
const V1 = `/api/graftyard/v1/repos/${R}`;
// A task per agent where the caps allow: TASKS_OPEN_PER_REQUESTER (100) per requester, or per App with none.
// Requesters for the tasks: TASKS_OPEN_PER_REQUESTER (100) each, so L0-12's task per agent needs five. Users
// `a1-u0`.. are seeded as tests/m3/hosted/setup.ts seeds m3-member (ids from 7100010), idempotent.
async function seedRequesters(n) {
  const { OP, must, accountShard, loginShard } = await import("../hosted/lib.mjs");
  const TRI = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 5 };
  const out = [];
  for (let k = 0; k < n; k++) {
    const [a, login] = [7100010 + k, `a1-u${k}`];
    await must(`names:${accountShard(a)}`, { cred: OP, op: { PutAccount: { id: a, suspended: false, sessions_not_before: 0 } } });
    await must(`names:${loginShard(login)}`, { cred: OP, op: { ClaimLogin: { login, kind: "User", account: a } } }, "Taken");
    await must(`names:${accountShard(a)}`, { cred: OP, op: { SetAccountLogin: { account: a, login } } });
    await must(`names:${accountShard(a)}`, { cred: OP, op: { InitPlacement: { owner: a, record: TRI } } }, "RecordExists");
    await must(`authz:${a}`, { cred: OP, op: { Open: { kind: "User", enterprise: null } } }, "AlreadyOpened");
    await must(`identity:${a}`, { cred: OP, op: { Create: { login, kind: "User", created: Date.now() } } }, "AlreadyCreated");
    out.push(login);
  }
  return out;
}
if (plan.leases || plan.side.taskEdit) {
  const extra = plan.leases ? await seedRequesters(Math.max(0, Math.ceil(plan.agents / 100) - 3)) : [];
  const requesters = [null, "m3-member", "m3-admin", ...extra];
  const want = plan.leases ? plan.agents : Math.min(plan.agents, 100);
  for (const a of agents.slice(0, want)) {
    const requester = requesters[Math.floor(a.i / 100)];
    if (requester === undefined) break;
    const r = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `a1-${RUN}-${a.i}`, summary: "a1", ...(requester ? { requester } : {}) });
    if (r.status === 201) a.task = r.body.id;
  }
  writeRow(OUT, { step: "tasks", made: agents.filter((a) => a.task).length, want });
  log(`tasks ${agents.filter((a) => a.task).length}/${want}`);
}
if (plan.leases) {
  for (const a of agents.filter((x) => x.task)) {
    const r = await api("POST", `${V1}/leases`, app, { paths: [`a1/${a.group}/${a.i}/**`], mode: "shared", ttl_seconds: 600 }, { "x-graftyard-agent-session": a.task });
    if (r.status === 201) a.lease = r.body?.lease?.id;
  }
  writeRow(OUT, { step: "leases", held: agents.filter((a) => a.lease).length });
}
// Issue and PR targets for comments and labels, and B0-35's open PRs.
const issue = await api("POST", "/api/graphql", admin, { query: `query { repository(owner: "m3org", name: "${opt.repo}") { id issues(first: 1) { nodes { id number } } } }` });
const repoId = issue.body?.data?.repository?.id;
let target = issue.body?.data?.repository?.issues?.nodes?.[0];
if (!target) {
  const c = await api("POST", "/api/graphql", admin, { query: "mutation($i: CreateIssueInput!) { createIssue(input: $i) { issue { id number } } }", variables: { i: { repositoryId: repoId, title: "a1 target" } } });
  target = c.body?.data?.createIssue?.issue;
}
await api("POST", `/api/v3/repos/${R}/labels`, admin, { name: "a1", color: "ededed" });
const openPrs = [];
const openPr = async (tok, branch, draft) => {
  const r = await api("POST", "/api/graphql", tok, { query: "mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { id number } } }",
    variables: { i: { repositoryId: repoId, baseRefName: "main", headRefName: branch, title: branch, body: "a1", draft } } });
  return [r, r.body?.data?.createPullRequest?.pullRequest];
};
if (plan.openPrs) {
  // Ten at a time: about 500 x 2 s / 10, under 3 minutes. A push or open that fails is retried up to 4 times, 2 s
  // apart, so a 503 under the setup's own load does not drop the PR; each failure's answer is counted.
  const why = {};
  const note = (k) => { why[k] = (why[k] ?? 0) + 1; };
  for (let b = 0; b < plan.openPrs; b += 10) {
    await Promise.all(Array.from({ length: Math.min(10, plan.openPrs - b) }, async (_, j) => {
      const ref = `refs/heads/claude/a1-${RUN}-pr${b + j}`;
      let pushed = false;
      let tipAt = null;
      for (let t = 0; t < 5 && !pushed; t++) {
        const [res, , nu] = await push(app, ref, null, `a1/pr/${b + j}.txt`, randomBytes(2_000));
        pushed = res === "ok";
        tipAt = nu;
        if (!pushed) { note(`push ${String(res).slice(0, 40)}`); await sleep(2_000); }
      }
      if (!pushed) return;
      for (let t = 0; t < 5; t++) {
        const [r, pr] = await openPr(app, ref.slice(11), true);
        if (pr) { openPrs.push({ ...pr, ref, tip: tipAt }); return; }
        note(`open ${r.status} ${JSON.stringify(r.body?.errors?.[0]?.message ?? r.body?.message ?? "").slice(0, 60)}`);
        await sleep(2_000);
      }
    }));
  }
  writeRow(OUT, { step: "open PRs", opened: openPrs.length, failures: why });
  log(`open PRs ${openPrs.length}/${plan.openPrs} ${JSON.stringify(why).slice(0, 400)}`);
  // Each of the first agents pushes its PR's head branch, so every push is a head push that asks for a test merge.
  for (const [k, pr] of openPrs.slice(0, plan.agents).entries()) { agents[k].ref = pr.ref; agents[k].tip = pr.tip; }
}

t0 = Date.now() + 5_000;
end = t0 + seconds * 1000;

// The scheduler: each kind at its rate, Poisson, with at most `cap` in flight per kind.
const inflight = {};
function every(kind, perS, f, cap = 400) {
  if (!perS) return Promise.resolve();
  return (async () => {
    await sleep(Math.max(0, t0 - Date.now()));
    while (Date.now() < end) {
      if ((inflight[kind] ?? 0) < cap) {
        inflight[kind] = (inflight[kind] ?? 0) + 1;
        const s = Date.now();
        f().then((code) => note(kind, Date.now() - s, code)).catch((e) => note(kind, Date.now() - s, `error ${String(e).slice(0, 30)}`))
          .finally(() => { inflight[kind]--; });
      } else note(kind, 0, "skipped: cap");
      await sleep(-Math.log(1 - Math.random()) * 1000 / perS);
    }
  })();
}
const pick = (xs) => xs[Math.floor(Math.random() * xs.length)];
const st = (r) => String(r.status);
// A test edge's `server-timing` on each answered API read: each stage's ms, kept as `api:<stage>` (narrow-reads-2).
const stages = (r) => {
  if (r.status === 200) for (const m of (r.headers?.["server-timing"] ?? "").matchAll(/([a-z_]+);dur=(\d+)/g)) note(`api:${m[1]}`, Number(m[2]), "200");
  return st(r);
};
let pushN = 0;
let labelN = 0;
const jobs = [
  // Each agent pushes every pushEvery s, its start spread over the first interval.
  ...agents.map((a) => (async () => {
    await sleep(Math.max(0, t0 - Date.now()) + Math.random() * plan.pushEvery * 1000);
    while (Date.now() < end) {
      const s = Date.now();
      const kill = plan.killEvery && ++pushN % Math.max(1, Math.round(plan.killEvery * plan.agents / plan.pushEvery)) === 0;
      const [res, ms, nu] = await push(app, a.ref, a.tip, `a1/${a.group}/${a.i}/f.txt`, randomBytes(size()), kill ? { "x-graft-kill": "after_quorum" } : {});
      a.tip = nu;
      note(kill ? "push:killed" : "push:agent", ms, res);
      await sleep(Math.max(0, Math.min(end - Date.now(), plan.pushEvery * 1000 - (Date.now() - s))));
    }
  })()),
  every("side:taskEdit", plan.side.taskEdit, async () => { const a = pick(agents.filter((x) => x.task)); return a ? st(await api("PATCH", `${V1}/tasks/${a.task}`, app, { summary: `a1 ${Date.now()}` })) : "no task"; }),
  every("side:comment", plan.side.comment, async () => {
    const r = await api("POST", "/api/graphql", app, { query: "mutation($i: AddCommentInput!) { addComment(input: $i) { subject { id } } }", variables: { i: { subjectId: target.id, body: "a1" } } });
    return r.body?.data?.addComment ? "ok" : `gql ${String(r.body?.errors?.[0]?.message ?? r.status).slice(0, 40)}`;
  }),
  every("side:label", plan.side.label, async () => {
    const add = (labelN++ & 1) === 0;
    return st(await api(add ? "POST" : "DELETE", `/api/v3/repos/${R}/issues/${target.number}/labels${add ? "" : "/a1"}`, app, add ? ["a1"] : undefined));
  }),
  every("side:standIn", plan.side.standIn, async () => { const a = pick(agents.filter((x) => x.task)); return a ? st(await api("PATCH", `${V1}/tasks/${a.task}`, app, { summary: `a1 stand-in ${Date.now()}` })) : "no task"; }),
  every("side:draftPr", plan.side.draftPr, async () => {
    const a = pick(agents.filter((x) => x.tip && !x.pr));
    if (!a) return "no branch";
    a.pr = true;
    const [r, pr] = await openPr(app, a.ref.slice(11), true);
    return pr ? "ok" : `gql ${String(r.body?.errors?.[0]?.message ?? r.status).slice(0, 40)}`;
  }),
  every("side:lease", plan.side.leaseAcquire + plan.side.leaseRelease, async () => {
    const a = pick(agents.filter((x) => x.task));
    if (!a) return "no task";
    const h = { "x-graftyard-agent-session": a.task };
    if (a.lease && !plan.leases) { const r = await api("DELETE", `${V1}/leases/${a.lease}`, app, undefined, h); a.lease = null; return st(r); }
    const r = await api("POST", `${V1}/leases`, app, { paths: [`a1/${a.group}/${a.i}/x/**`], mode: "shared", ttl_seconds: 600 }, h);
    if (r.status === 201 && !plan.leases) a.lease = r.body?.lease?.id;
    return st(r);
  }),
  every("side:leaseRenew", plan.side.leaseRenew, async () => {
    const a = pick(agents.filter((x) => x.lease));
    return a ? st(await api("POST", `${V1}/leases/${a.lease}/renew`, app, { ttl_seconds: 600 }, { "x-graftyard-agent-session": a.task })) : "no lease";
  }),
  every("push:human", plan.human / 2, async () => { const k = Math.floor(Math.random() * 20); const ref = `refs/heads/a1h-${RUN}-${k}`; const [res] = await push(admin, ref, await tip(ref), `a1h/${k}.txt`, randomBytes(20_000)); return res; }, 5),
  every("side:human", plan.human / 2 + plan.bot, async () => {
    const r = await api("POST", "/api/graphql", admin, { query: "mutation($i: AddCommentInput!) { addComment(input: $i) { subject { id } } }", variables: { i: { subjectId: target.id, body: "a1 human" } } });
    return r.body?.data?.addComment ? "ok" : `gql ${r.status}`;
  }),
  every("merge", plan.mergesPerMin / 60, async () => {
    const ref = `refs/heads/a1m-${RUN}-${Date.now().toString(36)}`;
    const [res] = await push(admin, ref, null, `a1m/${Date.now()}.txt`, randomBytes(5_000));
    if (res !== "ok") return `push ${res}`;
    const [, pr] = await openPr(admin, ref.slice(11), false);
    if (!pr) return "no pr";
    const s = Date.now();
    const m = await api("PUT", `/api/v3/repos/${R}/pulls/${pr.number}/merge`, admin, { merge_method: "merge" });
    note("merge:only", Date.now() - s, st(m));
    if (m.status === 200) main = (await tip("refs/heads/main")) ?? main;
    return st(m);
  }, 3),
  every("read:ref", plan.refReads, async () => st(await lsRefs(admin, "refs/heads/"))),
  every("read:api", plan.apiReads, async () => stages(Math.random() < 0.5
    ? await api("GET", `/api/v3/repos/${R}/issues?per_page=30`, admin)
    : await api("POST", "/api/graphql", admin, { query: `query { repository(owner: "m3org", name: "${opt.repo}") { pullRequests(last: 10) { nodes { number isDraft${plan.openPrs ? " mergeable" : ""} } } } }` }))),
  every("read:clone", plan.clones, async () => {
    const s = Date.now();
    const r = await fetch(`${EDGE}/${R}.git/git-upload-pack`, { method: "POST", headers: { ...auth(admin), "git-protocol": "version=2", "content-type": "application/x-git-upload-pack-request" },
      body: Buffer.concat([pkt("command=fetch\n"), Buffer.from("0001"), pkt("no-progress\n"), pkt(`want ${main}\n`), pkt("done\n"), Buffer.from("0000")]) });
    const reader = r.body.getReader();
    await reader.read();
    note("read:cloneTtfb", Date.now() - s, String(r.status));
    while (!(await reader.read()).done);
    return String(r.status);
  }, 20),
  every("fast-user", (plan.fastUser ?? 0) / 60, async () => st(await api("POST", "/api/graphql", tokens.member, { query: "mutation($i: AddCommentInput!) { addComment(input: $i) { subject { id } } }", variables: { i: { subjectId: target.id, body: "a1 fast" } } }))),
  every("read:mergeable", plan.prPollers ? plan.prPollers * (1 - restPolls) / plan.prPollEvery : 0, async () => {
    const pr = pick(openPrs);
    const r = await api("POST", "/api/graphql", app, { query: `query { repository(owner: "m3org", name: "${opt.repo}") { pullRequest(number: ${pr.number}) { mergeable } } }` });
    return r.body?.data?.repository?.pullRequest?.mergeable ?? `http ${r.status}`;
  }),
  // REST pollers (`--rest-polls`): GHES 3.22 answers `mergeable` null here until a merge is computed, so the answer
  // recorded is the status and whether the field was null; the shard's `getAsked` counts their asks.
  every("read:mergeableRest", plan.prPollers ? plan.prPollers * restPolls / plan.prPollEvery : 0, async () => {
    const pr = pick(openPrs);
    const r = await api("GET", `/api/v3/repos/${R}/pulls/${pr.number}`, app);
    return r.status === 200 ? `200 mergeable=${r.body?.mergeable}` : `http ${r.status}`;
  }),
  (async () => { while (Date.now() < end) { await sleep(Math.min(40 * 60_000, end - Date.now())); if (Date.now() < end) await refreshApp(); } })(),
];
if (plan.burst) jobs.push((async () => {
  await sleep(Math.max(0, t0 + plan.burst.at * 1000 - Date.now()));
  const s = Date.now();
  const r = await Promise.all(Array.from({ length: plan.burst.pushes }, (_, k) => sleep((k * 1000) / plan.burst.pushes).then(async () => {
    const a = agents[k % agents.length];
    const [res, ms] = await push(app, `${a.ref}-b`, null, `a1/${a.group}/${a.i}/burst.txt`, randomBytes(plan.burst.bytes));
    note("push:burst", ms, res);
    return [res, Date.now() - s];
  })));
  writeRow(OUT, { step: "burst", pushes: plan.burst.pushes, within5s: r.filter(([x, t]) => x === "ok" && t <= 5_000).length, results: r.reduce((m, [x]) => ((m[x] = (m[x] ?? 0) + 1), m), {}) });
})());
if (opt["kill-shard-at"]) jobs.push((async () => {
  await sleep(Math.max(0, t0 + Number(opt["kill-shard-at"]) * 1000 - Date.now()));
  const { bedGet } = await import("../hosted/lib.mjs");
  const id = CHAIN.split(":")[1];
  const r = await bedGet(`/_bed/kill?repo=${id}`);
  writeRow(OUT, { step: "shard kill", status: r.status, at: Date.now() - t0 });
})());

// The feed: pollers straight to followers and learners, as the edge sends them, each watching one agent group.
const seen = []; // [poller, ref, ms after its push's ack]
if (plan.pollers) {
  const cfg = (await statuses(CHAIN)).find((s) => s.config)?.config?.config?.normal;
  const learners = (cfg?.learners ?? []).map((l) => ({ site: l.site, name: l.do_name }));
  const followers = (await statuses(CHAIN)).filter((s) => s.role === "follower").map((s) => ({ site: s.site, name: undefined }));
  const serve = learners.length ? learners : followers;
  writeRow(OUT, { step: "feed members", learners: learners.length, followers: followers.length });
  for (let k = 0; k < plan.pollers; k++) {
    jobs.push((async () => {
      await sleep(Math.max(0, t0 - Date.now()) + (k % 500) * 4);
      let cursor = null;
      const m = serve[k % serve.length];
      while (Date.now() < end) {
        const s = Date.now();
        try {
          const r = await member(CHAIN, m.site, "/feed", { cursor, wait_s: 25, paths: [`a1/${k % 50}/**`], learner_answers: !!learners.length,
            token: `a1-${RUN}-${k}`, ip: `a1ip-${k}` }, 40_000, m.name);
          const v = r.v ?? {};
          note("feed", Date.now() - s, v.admit ?? `status ${r.status}`);
          if (v.cursor) cursor = v.cursor;
          for (const e of v.events ?? []) {
            const p = e[2]?.AgentPush;
            const head = p ? (typeof p.head === "string" ? p.head : Buffer.from(p.head).toString("hex")) : null;
            if (head && pushed.has(head)) seen.push([k, head, Date.now() - pushed.get(head)]);
          }
          if (v.admit && v.admit !== "hold") await sleep(1000);
        } catch (e) { note("feed", Date.now() - s, "error"); await sleep(1000); }
      }
    })());
  }
}

// B0-35's instruments: each site's shard of the network, its test-merge budget's counts (null before any merge).
const budgets = () => Promise.all([0, 1, 2].map(async (s) => {
  try { return JSON.parse((await bedGet(`/_bed/shard_status?repo=${CHAIN.split(":")[1]}&shard=${s}`)).body).testMerges ?? null; } catch { return null; }
}));
const budgetBefore = plan.openPrs ? await budgets() : null;
// The shard object keeps its counts in memory, and hosted it restarted 10 to 30 s after the load stopped
// (`tests/am1/results/merge-budget-2.md`), so the run samples them every 15 s and keeps the latest of each site.
const budgetSeen = plan.openPrs ? [...budgetBefore] : null;
/** Each site's counts after the window: the read now, or, for a site whose object restarted since (a later `since`
 *  with fewer merges, or none), the last sample of the run. */
function latestBudget(now, seen) {
  return now.map((b, s) => (!b || (seen[s] && b.since !== seen[s].since && b.computed < seen[s].computed)) ? seen[s] ?? b : b);
}
const leaderBefore = await statuses(CHAIN);
const leader = leaderBefore.find((s) => s.role === "leader")?.site;
if (leader !== undefined) await member(CHAIN, leader, "/stats?reset=1", undefined, 15_000);
// `--first-for`: round 5's concurrent re-sends, on purpose. Every member gets it, so a leader elected mid-run has it too.
if (opt["first-for"]) for (const s of [0, 1, 2]) {
  const v = (await member(CHAIN, s, "/bench_seed", { prsyncFirstFor: Number(opt["first-for"]) }, 15_000).catch((e) => ({ v: String(e) }))).v;
  log(`site ${s} prsyncFirstFor ${JSON.stringify(v)}`);
}
log(`${row} run ${RUN}: ${plan.agents} agents, ${seconds} s from ${new Date(t0).toISOString()}, leader site ${leader}`);
if (plan.openPrs) jobs.push((async () => {
  while (Date.now() < end) {
    await sleep(15_000);
    (await budgets()).forEach((b, s) => { if (b && (!budgetSeen[s] || b.since === budgetSeen[s].since || b.computed >= budgetSeen[s].computed)) budgetSeen[s] = b; });
  }
})());
await Promise.all(jobs);
if (opt["first-for"]) for (const s of [0, 1, 2]) await member(CHAIN, s, "/bench_seed", { prsyncFirstFor: 0 }, 15_000).catch(() => {});
// Give the last pushes' first attempts time to reach the shard.
const budgetAfter = plan.openPrs ? latestBudget((await sleep(15_000), await budgets()), budgetSeen) : null;
const after = await statuses(CHAIN);
// A leader mid-reset answers `/stats` with no body; the row still writes its own counts.
const lstats = (leader !== undefined ? (await member(CHAIN, leader, "/stats", undefined, 15_000).catch(() => ({}))).v : null) ?? {};
const sum = Object.fromEntries(Object.entries(lat).map(([k, xs]) => [k, { n: xs.length, perS: +(xs.length / seconds).toFixed(2), p50: pct(xs, 0.5), p99: pct(xs, 0.99), codes: codes[k] }]));
const feedLat = seen.map((x) => x[2]);
const v = {
  row, run: RUN, seconds, agents: plan.agents, offered: offered(plan), byKind: sum,
  feed: { events: seen.length, p50: pct(feedLat, 0.5), p99: pct(feedLat, 0.99), max: feedLat.length ? Math.max(...feedLat) : null },
  leader: { before: leader, after: after.find((s) => s.role === "leader")?.site, terms: after.map((s) => [s.site, s.role, s.term]),
    commitMs: { n: lstats.commitMs?.length, p50: pct(lstats.commitMs, 0.5), p99: pct(lstats.commitMs, 0.99) },
    // Round 5's commit split: admission to append (`queueMs`, which holds the entry's dep reads), the dep reads by
    // chain kind (`depsMs`), append to apply (`roundMs`: storage confirm and quorum), and the confirm alone.
    split: Object.fromEntries([["queue", lstats.queueMs], ["round", lstats.roundMs], ["confirm", lstats.confirmMs],
      ...Object.entries(lstats.depsMs ?? {}).map(([k, xs]) => [`deps:${k}`, xs])]
      .map(([k, xs]) => [k, { n: xs?.length, p50: pct(xs, 0.5), p99: pct(xs, 0.99) }])),
    depFails: lstats.depFails,
    reqPerS: lstats.inboundMs ? +(Object.values(lstats.inbound ?? {}).reduce((a, b) => a + b, 0) / (lstats.inboundMs / 1000)).toFixed(1) : null,
    feedPollsOnLeader: lstats.inbound?.["/feed"] ?? 0, inbound: lstats.inbound, inboundMs: lstats.inboundMs, failedCalls: lstats.failedCalls },
  feedMembers: after.map((s) => ({ site: s.site, role: s.role, feed: s.feed })),
  ...(plan.openPrs ? { testMerges: { ...budgetWindow(budgetBefore, budgetAfter, seconds), leader: lstats.testMerges ?? null,
    pollAnswers: codes["read:mergeable"] ?? {}, restPollAnswers: codes["read:mergeableRest"] ?? {},
    firstFor: opt["first-for"] ? Number(opt["first-for"]) : null } } : {}),
};
writeRow(OUT, v);
log(JSON.stringify(v).slice(0, 4000));
// Pushes and polls cut off at the row's end can hold sockets open for minutes, which stalled `rows.sh` on B0-29 and
// B0-31 (`tests/am1/results/lm1-memory.md`, section 10): the row is written, so end here.
process.exit(0);
