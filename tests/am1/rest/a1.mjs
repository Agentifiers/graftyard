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
//     [--b034 FILE]      # B0-34 as written, on the seeded 5 GB monorepo (tests/fanout/b034.sh). FILE is JSON:
//                        # `bare` (the local torvalds/linux clone) and `future` (the first-parent commits after the
//                        # seeded main), which replace the PR merges: 10 a minute push the next one to main, so the
//                        # tip moves along real history; `second` {repo, chain, who}, a network of another owner made
//                        # hot from `secondAt` s; `hostile` [{repo, chain, who}], three networks of a third owner made hot from
//                        # `hostileAt` s; `hotPushPerS` and `hotClonePerS` per hot network (M0: 1 and 3). Clones are
//                        # depth 1 and stop at the first byte (5 GB a clone cannot cross the orb). k of every network
//                        # is read every 30 s. No Cloudflare API call is the static check in b034.sh: no request path
//                        # can reach the API, so "unreachable" holds by construction (`design/DESIGN.md`, Invariants).
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
import { Agent as HttpsAgent, request as httpsRequest } from "node:https";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { deflateSync } from "node:zlib";
import { parseArgs } from "node:util";
import { clockSpread, dateBounds, driverOffset } from "./clock.mjs";
import { r2list } from "../../fanout/r2list.mjs";

const { values: opt, positionals } = parseArgs({ allowPositionals: true, strict: true, options: {
  dry: { type: "boolean" }, selftest: { type: "boolean" }, seconds: { type: "string" }, agents: { type: "string" },
  repo: { type: "string" }, chain: { type: "string" }, "kill-every": { type: "string" }, "kill-shard-at": { type: "string" },
  run: { type: "string" }, "api-reads": { type: "string" }, "ref-reads": { type: "string" }, pollers: { type: "string" },
  "push-every": { type: "string" }, "as-admin": { type: "boolean" }, "rest-polls": { type: "string" },
  "first-for": { type: "string" }, b034: { type: "string" }, "open-prs": { type: "string" }, workflow: { type: "boolean" } } });

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
    // Buckets up for the first half, then placement failing (`x-graft-place-fail`) for the second.
    case "b0-31": return { ...base, packFixed: 1 << 20, placeFailAt: seconds / 2 };
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

const FIELDS = ["spentMs", "wallMs", "computed", "shed", "late", "getAsked", "getComputes", "listAsked", "listComputes"];
// `graftyard_git::prsync::WAIT_EDGES_MS`: the upper edges of `waitHist`'s buckets, then one bucket past the last.
export const WAIT_EDGES_MS = [10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000, 20000, 30000, 60000, 120000];
const COALESCE_MS = 30000;
/** A merge-wait histogram's quantile `q`, as its bucket's upper edge (or `maxMs` past the last), and the count past
 * GET_COALESCE_MS; as `BudgetStats::wait_ms` and `waits_over`. */
export function waitSummary(hist, maxMs) {
  const total = hist.reduce((n, c) => n + c, 0);
  const at = (q) => {
    if (!total) return null;
    const rank = Math.max(1, Math.ceil(q * total));
    let seen = 0;
    for (let i = 0; i < hist.length; i++) { seen += hist[i]; if (seen >= rank) return i < WAIT_EDGES_MS.length ? Math.min(WAIT_EDGES_MS[i], maxMs) : maxMs; }
    return maxMs;
  };
  const over = hist.slice(WAIT_EDGES_MS.indexOf(COALESCE_MS) + 1).reduce((n, c) => n + c, 0);
  return { merges: total, p50Ms: at(0.5), p99Ms: at(0.99), maxMs, overCoalesceMs: over };
}
/** The window's counts per site: `after` less `before`, or `after` alone if the shard object restarted (`since`). */
export function budgetWindow(before, after, seconds) {
  const sites = after.map((a, site) => {
    if (!a) return null;
    const b = before[site];
    const restarted = !b || b.since !== a.since;
    // `lowMs` and `maxMs` are the bucket's lowest and the largest charge since `since`, so they cover the window only
    // on a shard that restarted for it; otherwise they are an upper bound on the overrun over the shard's life.
    const hist = WAIT_EDGES_MS.concat([0]).map((_, i) => (a.waitHist?.[i] ?? 0) - (restarted ? 0 : b.waitHist?.[i] ?? 0));
    return { site, vcpu: a.vcpu, restarted, lowMs: a.lowMs ?? 0, maxMs: a.maxMs ?? 0, inFlightMax: a.inFlightMax ?? 0,
      waitHist: hist, waitMaxMs: a.waitMaxMs ?? 0,
      ...Object.fromEntries(FIELDS.map((f) => [f, (a[f] ?? 0) - (restarted ? 0 : b[f] ?? 0)])) };
  }).filter(Boolean);
  const sum = (f) => sites.reduce((n, x) => n + x[f], 0);
  const vcpuUsed = +(sum("spentMs") / (seconds * 1000)).toFixed(4);
  const budgetVcpu = sites.length ? Math.max(...sites.map((x) => x.vcpu)) : null;
  // Overrun beyond one merge: the bucket went lower than the largest single charge (or the 50 ms estimate) below zero.
  const overrun = sites.some((x) => x.lowMs < -Math.max(x.maxMs, 50) - 0.5);
  return { sites, vcpuUsed, budgetVcpu, withinBudget: budgetVcpu !== null && vcpuUsed <= budgetVcpu, overrun,
    inFlightMax: Math.max(0, ...sites.map((x) => x.inFlightMax)),
    // `waitMaxMs` covers the shard's life since `since`, as `lowMs` does: the window's own max is at most it.
    wait: waitSummary(WAIT_EDGES_MS.concat([0]).map((_, i) => sites.reduce((n, x) => n + x.waitHist[i], 0)),
      Math.max(0, ...sites.map((x) => x.waitMaxMs))),
    computed: sum("computed"), shed: sum("shed"), late: sum("late"), getAsked: sum("getAsked"), getComputes: sum("getComputes"),
    listAsked: sum("listAsked"), listComputes: sum("listComputes"), cpuToWall: sum("wallMs") ? +(sum("spentMs") / sum("wallMs")).toFixed(3) : null };
}

/** B0-35's dispatch line: each PR's head-push acks in the window paired in order with that PR's runs first seen after
 *  them; a run with no ack before it (setup's `opened`, or a PR opened in the window) is `unpaired`, and an ack with no
 *  run after it is `missing` (a conflict, or a run not created). Times are from the ack to the poll that first listed
 *  the run, so each is late by up to one poll. Nearest-rank percentiles, as the row's others. */
export function dispatchLine(acks, runs, t0, pollMs, polls = {}) {
  const at = (xs, q) => { const s = [...xs].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(q * s.length))] : null; };
  const by = (xs) => xs.reduce((m, x) => ((m[x.n] ??= []).push(x), m), {});
  const A = by(acks.filter((a) => a.at >= t0)), Rn = by(runs.filter((r) => r.seenAt >= t0 && Number.isFinite(r.n)));
  const ms = [];
  let missing = 0, unpaired = 0;
  for (const n of new Set([...Object.keys(A), ...Object.keys(Rn)])) {
    const as = (A[n] ?? []).sort((x, y) => x.at - y.at), rs = (Rn[n] ?? []).sort((x, y) => x.seenAt - y.seenAt);
    let j = 0;
    for (const a of as) {
      while (j < rs.length && rs[j].seenAt < a.at) { unpaired++; j++; }
      if (j < rs.length) ms.push(rs[j++].seenAt - a.at); else missing++;
    }
    unpaired += rs.length - j;
  }
  return { pushes: acks.filter((a) => a.at >= t0).length, paired: ms.length, missing, unpaired,
    setupRuns: runs.filter((r) => r.seenAt < t0).length, runsSeen: runs.length,
    p50: at(ms, 0.5), p99: at(ms, 0.99), max: ms.length ? Math.max(...ms) : null, pollMs, polls };
}

// ---- L0-12's lease overlap ----

/** Probe pairs: each pair hands one Exclusive lease (`a1p/<pair>/**`, PROBE_TTL_S, never renewed) back and forth. */
export const PROBE_PAIRS = 20;
export const PROBE_TTL_S = 60;
/** How often the prober asks while the holder's lease is near its end. */
export const PROBE_EVERY_MS = 200;
const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/** The ms a lease id (a ULID, `graftyard_git::lease::next_id`) was minted at: the granting leader's entry `at`. */
export function ulidMs(id) {
  let t = 0;
  for (const c of String(id).slice(0, 10).toUpperCase()) {
    const v = CROCKFORD.indexOf(c);
    if (v < 0) return null;
    t = t * 32 + v;
  }
  return t;
}
/** How long two holders held the same Exclusive scope: the earlier lease's end (its grant `at` plus its TTL, as
 *  `IssueOp::AcquireLease` writes `expires`) less the later grant's `at`, both stamped by the leaders that granted
 *  them. Positive is an overlap; zero or less is none. */
export function overlapMs(heldId, ttlS, grantedId) {
  const a = ulidMs(heldId), b = ulidMs(grantedId);
  return a === null || b === null ? null : a + ttlS * 1000 - b;
}

// ---- packs: one blob, its tree, and a commit, as git writes them ----

/** B0-35's workflow (`--workflow`): one job per `pull_request` event, on a label no runner has, so no runner, hosted or
 *  self-hosted, ever takes it and `graft-runners` starts no Sandbox; the driver cancels every run after the window. */
export const WF_PATH = ".github/workflows/b035.yml";
export const WF_SRC = Buffer.from("on: pull_request\njobs:\n  t:\n    runs-on: [self-hosted, b035-dispatch]\n    steps:\n      - run: \"true\"\n");

const sha1 = (b) => createHash("sha1").update(b).digest();
const obj = (type, body) => sha1(Buffer.concat([Buffer.from(`${type} ${body.length}\0`), body]));
const TYPE = { commit: 1, tree: 2, blob: 3 };
/** merge:only's time by stage (`merge-p99`): each `server-timing` and `x-graft-shard-timing` stage's n, p50, p99
 * and max over the merges that carried it, and `gap`, what `total` leaves after `auth`, the longer of `tier` and
 * `authz` (they run at once), `merge_plan`, and `commit`: an untimed stage. Nearest rank, as the run's other p99s. */
export function mergeSplit(steps) {
  const by = {};
  const add = (k, v) => (by[k] ??= []).push(v);
  for (const m of steps) {
    const d = Object.fromEntries([...(m.timing ?? "").matchAll(/([a-z_]+);dur=(\d+)/g)].map((x) => [x[1], Number(x[2])]));
    for (const [k, v] of Object.entries(d)) add(k, v);
    for (const x of (m.shard ?? "").matchAll(/([a-z_]+);dur=(\d+)/g)) add(`shard:${x[1]}`, Number(x[2]));
    if (d.total !== undefined) add("gap", d.total - (d.auth ?? 0) - Math.max(d.tier ?? 0, d.authz ?? 0) - (d.merge_plan ?? 0) - (d.commit ?? 0));
  }
  const at = (xs, q) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(q * s.length))]; };
  return Object.fromEntries(Object.entries(by).map(([k, xs]) => [k, { n: xs.length, p50: at(xs, 0.5), p99: at(xs, 0.99), max: Math.max(...xs) }]));
}

function entryHeader(type, size) {
  const out = [];
  let b = (TYPE[type] << 4) | (size & 15);
  size >>= 4;
  while (size) { out.push(b | 0x80); b = size & 0x7f; size >>= 7; }
  out.push(b);
  return Buffer.from(out);
}
/** The tree of `files` ([path parts, blob id]), its subtrees pushed to `objects`; entries sorted as git sorts them
 *  (a directory's name compares as if it ended in "/"). */
function treeOf(files, objects) {
  const groups = new Map();
  for (const [[h, ...rest], id] of files) (groups.get(h) ?? groups.set(h, []).get(h)).push([rest, id]);
  const entries = [...groups].map(([name, xs]) => (xs.length === 1 && xs[0][0].length === 0
    ? { mode: "100644", name, id: xs[0][1] } : { mode: "40000", name, id: treeOf(xs, objects) }));
  const key = (e) => Buffer.from(e.mode === "40000" ? `${e.name}/` : e.name);
  entries.sort((a, b) => Buffer.compare(key(a), key(b)));
  const body = Buffer.concat(entries.map((e) => Buffer.concat([Buffer.from(`${e.mode} ${e.name}\0`), e.id])));
  objects.push(["tree", body]);
  return obj("tree", body);
}
/** The pack and the commit id of one commit on `parent` (hex, or null) holding `bytes` at `path`, and each `extra`
 *  [path, bytes] beside it (B0-35's workflow file, so every commit and every test merge keeps it). */
export function onePack(path, bytes, parent, message, at = 1_700_000_000, extra = []) {
  const files = [[path, bytes], ...extra];
  const blobs = files.map(([, b]) => ["blob", b]);
  const objects = [];
  const root = treeOf(files.map(([p, b]) => [p.split("/"), obj("blob", b)]), objects);
  const who = `a1 <a1@graftyard.invalid> ${at} +0000`;
  const commit = Buffer.from(`tree ${root.toString("hex")}\n${parent ? `parent ${parent}\n` : ""}author ${who}\ncommitter ${who}\n\n${message}\n`);
  const all = [...blobs, ...objects, ["commit", commit]];
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
  // Any other refusal keeps its reason, ids and numbers masked, so a row says why (B0-31's 1,484 were unexplained).
  const mask = (x) => x.replace(/[0-9a-f]{12,}/g, "H").replace(/\d+/g, "N").slice(0, 80).trim();
  // `unpacker error` names its cause on the `unpack <why>` line (`Refusal::unpack_failed`).
  const unpack = lines.map((l) => l.match(/^unpack (?!ok\n)([^\n]*)/)).find(Boolean);
  if (ng) return ng[2].includes("admission full") ? "full" : /stale|fetch first|non-fast-forward/.test(ng[2]) ? "stale"
    : `refused ${mask(ng[2])}${unpack ? `: ${mask(unpack[1])}` : ""}`;
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
    const wf = onePack("a1/3/7/f.txt", Buffer.from("third\n"), b.commit, "third", 1_700_000_000, [[WF_PATH, WF_SRC]]);
    execFileSync("git", ["index-pack", "--stdin", "--strict"], { cwd: dir, input: wf.pack });
    execFileSync("git", ["fsck", "--strict", "--no-dangling", wf.commit], { cwd: dir });
    const cat = (spec) => execFileSync("git", ["cat-file", "-p", spec], { cwd: dir }).toString();
    const ok = log === "second\nfirst" && cat(`${b.commit}:a1/3/7/f.txt`) === "second\n"
      && cat(`${wf.commit}:${WF_PATH}`) === WF_SRC.toString() && cat(`${wf.commit}:a1/3/7/f.txt`) === "third\n";
    const s = (...ls) => Buffer.concat([...ls.map(pkt), Buffer.from("0000")]).toString("latin1");
    const r = pushResult(s("unpack ok\n", "ok refs/heads/claude/a1-x-1\n"));
    // A length ending in a hex letter: `002cok refs/...` (am1-runs' and fanout's B0-35 setup read these as errors).
    const hex = pushResult(s("unpack ok\n", "ok refs/heads/claude/a1-mwa4unw0-pr10\n"));
    const n = pushResult(s("unpack ok\n", "ng refs/heads/x agent admission full, retry after 2s\n"));
    const why = pushResult(s("unpack ok\n", "ng refs/heads/x pack 1048576 bytes over INLINE_MAX_HOT 2\n"));
    const unp = pushResult(s("unpack validate: shard 503 after 30000 ms\n", "ng refs/heads/x unpacker error\n"));
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
    const h = (m) => WAIT_EDGES_MS.concat([0]).map((_, i) => m[i] ?? 0);
    const waitWin = budgetWindow([bs(1, 0, 0, 0, { waitHist: h({ 0: 50, 11: 1 }) }), null],
      [bs(1, 0, 0, 0, { waitHist: h({ 0: 147, 11: 2, 12: 2 }), waitMaxMs: 70000 }), null], 60);
    const win = w.sites.length === 2 && w.computed === 54 && w.shed === 4 && w.vcpuUsed === 0.1067 && w.withinBudget && w.sites[1].restarted
      && w.getAsked === 20 && w.getComputes === 4 && w.listAsked === 7 && w.cpuToWall === 0.5 && !w.overrun
      && old.getAsked === 0 && !old.overrun && over.overrun && w.inFlightMax === 3 && old.inFlightMax === 0
      // Waits: the window's delta on a kept shard, all of a restarted one; 2 of 100 past 30 s.
      && JSON.stringify(waitWin.wait) === JSON.stringify({ merges: 100, p50Ms: 10, p99Ms: 60000, maxMs: 70000, overCoalesceMs: 2 })
      && old.wait.merges === 0 && old.wait.p99Ms === null
      // A shard object that restarted after the load keeps the run's last sample; one still up reads now.
      && latestBudget([bs(9, 0, 0, 0), null, bs(1, 700, 7, 0)], [bs(1, 600, 6, 0), null, bs(1, 600, 6, 0)])
        .map((b) => b?.computed ?? null).join() === "6,,7";
    // Lease ids: 01ARYZ6S41 is 1469918176385 ms (`graftyard_types::Ulid::new` encodes it so); a grant 300 ms before
    // the held lease's end is a 300 ms overlap, and one 1 ms after it is none.
    const held = "01ARYZ6S41TSV4RRFFQ69G5FAV";
    const enc = (ms) => { let o = ""; for (let k = 0; k < 10; k++) { o = CROCKFORD[ms % 32] + o; ms = Math.floor(ms / 32); } return `${o}0000000000000000`; };
    const lease = ulidMs(held) === 1469918176385 && overlapMs(held, 60, enc(1469918176385 + 59_700)) === 300
      && overlapMs(held, 60, enc(1469918176385 + 60_001)) === -1;
    // L0-12's clock spread (clock.mjs). Round 1 is rows-m19's idle shape, three members within 60 ms on round trips of
    // 70, 180 and 240 ms; round 2's 3,000 ms round trip on site 0 is queueing and is rejected, not widened (widening
    // it read 3.2 s). Site 2's floor is 480 ms in the apac rounds, so 680 is kept and 681 rejected: the cap is over each
    // site's own floor. Round 5 is a real 900 ms skew: at least 850 and at most 950.
    const cs = (round, site, offset, rtt) => ({ round, site, offset, rtt });
    const idle = clockSpread([cs(1, 0, 8000, 70), cs(1, 1, 8030, 180), cs(1, 2, 8060, 240), cs(2, 0, 6500, 3000), cs(2, 1, 8040, 190)]);
    const apac = clockSpread([cs(3, 0, 0, 60), cs(3, 2, 10, 480), cs(4, 0, 0, 60), cs(4, 2, 10, 680), cs(4, 2, 1500, 681)]);
    const skew = clockSpread([cs(5, 0, 0, 50), cs(5, 1, 900, 50)]);
    const clock = JSON.stringify(idle) === JSON.stringify({ upperMs: 240, lowerMs: 0, rounds: 1, kept: 4, rejected: 1 })
      && JSON.stringify(apac) === JSON.stringify({ upperMs: 380, lowerMs: 0, rounds: 2, kept: 4, rejected: 1 })
      && skew.upperMs === 950 && skew.lowerMs === 850 && clockSpread([cs(6, 0, 0, 50)]).upperMs === null
      // The driver offset (clock.mjs `dateBounds`): Cloudflare 1,700 ms ahead of this orb. A probe at orb 10,000 to
      // 10,100 reads Date 11,000 (offset in [900, 2,000]); one at 10,350 to 10,450 reads 12,000 ([1,550, 2,650]); one
      // at 10,200 to 10,280 reads 11,000 ([720, 1,800]). Together: [1,550, 1,800], which holds the true 1,700. A probe
      // whose Date is two seconds off from the others empties the intersection.
      && JSON.stringify(dateBounds([{ s: 10_000, e: 10_100, date: 11_000 }, { s: 10_350, e: 10_450, date: 12_000 },
        { s: 10_200, e: 10_280, date: 11_000 }])) === JSON.stringify({ lo: 1550, hi: 1800, offsetMs: 1675, n: 3 })
      && dateBounds([{ s: 10_000, e: 10_100, date: 11_000 }, { s: 10_000, e: 10_100, date: 13_000 }]) === null;
    // merge:only's split: `tier` and `authz` overlap, so the gap counts the longer; a merge with no shard timing
    // adds no shard stage.
    const sp = mergeSplit([
      { timing: "resolve;dur=0, auth;dur=50, authz;dur=200, tier;dur=900, merge_plan;dur=150, plan;dur=0, commit;dur=500, total;dur=1900", shard: "merge;dur=80, waited;dur=0, catchup;dur=0" },
      { timing: "auth;dur=40, authz;dur=600, tier;dur=100, merge_plan;dur=100, commit;dur=400, total;dur=1140", shard: "" },
    ]);
    const split = sp.gap.n === 2 && sp.gap.max === 300 && sp.tier.max === 900 && sp["shard:merge"].n === 1 && sp.total.p99 === 1900;
    // B0-35's dispatch pairing: PR 1's two acks pair with its next runs (50 and 60 ms); its setup run (before t0) is
    // not counted; PR 2's ack has no run (missing); PR 3's run has no ack (unpaired); a run seen between two acks pairs
    // with the earlier.
    const dl = dispatchLine([{ n: 1, at: 100 }, { n: 1, at: 200 }, { n: 2, at: 300 }],
      [{ n: 1, seenAt: 40 }, { n: 1, seenAt: 150 }, { n: 1, seenAt: 260 }, { n: 3, seenAt: 120 }], 90, 1000);
    const disp = dl.paired === 2 && dl.p50 === 60 && dl.max === 60 && dl.missing === 1 && dl.unpaired === 1 && dl.setupRuns === 1;
    console.log(JSON.stringify({ packs: ok, ok: r, hex, full: n, band, budgetWindow: win, lease, split, clock, dispatch: disp }));
    if (!ok || r !== "ok" || hex !== "ok" || n !== "full" || band !== "ok" || !win || !lease || !split || !clock || !disp
      || why !== "refused pack N bytes over INLINE_MAX_HOT N" || unp !== "refused unpacker error: validate: shard N after N ms") process.exit(1);
  } finally { rmSync(dir, { recursive: true, force: true }); }
  process.exit(0);
}

const row = positionals[0];
const seconds = Number(opt.seconds ?? 600);
// `--api-reads` and `--ref-reads` override the row's read rates (a diagnostic run; the gate runs at the row's).
const plan = planFor(row, { agents: opt.agents ? Number(opt.agents) : undefined, seconds,
  apiReads: opt["api-reads"] !== undefined ? Number(opt["api-reads"]) : undefined, refReads: opt["ref-reads"] !== undefined ? Number(opt["ref-reads"]) : undefined });
if (opt["kill-every"]) plan.killEvery = Number(opt["kill-every"]);
// B0-34 as written: the tip moves along real history in place of PR merges (a merge commit would leave main off the
// history the next move fast-forwards to).
const b034 = opt.b034 ? JSON.parse((await import("node:fs")).readFileSync(opt.b034, "utf8")) : null;
if (b034) { plan.mergesPerMin = 0; plan.tipMovesPerMin = 10; }
if (opt.pollers !== undefined) plan.pollers = Number(opt.pollers);
// `--open-prs N`: N draft PRs opened in this run before its window, each on an agent's own head branch, so the first N
// agents' pushes are head pushes with their PrSync and test-merge records, as B0-29's row text has them. B0-35's
// mergeable pollers stay off. LM-1 round 12's agreed B0-29 shape is 500 (tests/am1/results/lm1-memory.md).
if (opt["open-prs"] !== undefined) plan.openPrs = Number(opt["open-prs"]);
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
const { api, bedGet, BED, ghs, log, member, row: writeRow, sleep, state, statuses, tokens, voterAt } = await import("../hosted/lib.mjs");
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
// B0-34 as written keeps each phase's pushes, merges, and clones apart (`b034Phase`).
const PHASED = /^(push:|read:clone|tip:move|hot:)/;
const note = (k, ms, code) => {
  if (b034 && PHASED.test(k)) k += b034Phase();
  (lat[k] ??= []).push(ms); const c = (codes[k] ??= {}); c[code] = (c[code] ?? 0) + 1;
};
const pct = (xs, p) => { const s = [...(xs ?? [])].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : null; };
const mergeSteps = [];
const humanInFlight = {};

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

// B0-31's placement-failing half: from `placeFailAt` s into the load, every push and merge carries the bed token's
// `x-graft-place-fail`, so the edge sees placement failing as a red bucket probe would.
const placeFailing = () => plan.placeFailAt !== undefined && Date.now() >= t0 + plan.placeFailAt * 1000;
// A sample's half is the one its request was sent in, as `failHeaders` decides at send: call `half` before the request.
// m3fr8 labelled agent pushes when they ended, and 17 sent with buckets up landed in `push:agent@fail` as commits.
const half = (k) => (plan.placeFailAt === undefined ? k : `${k}${placeFailing() ? "@fail" : "@up"}`);
const failHeaders = () => (placeFailing() ? { "x-graft-place-fail": "1", "x-bed-token": BED } : {});
// Past the edge's own worst case, so every push ends in the edge's answer and none in the driver's timeout: three
// shards' validate waits (15 + 60 + 60 s) then the commit. At 120 s, B0-32's burst pushes still in their third
// shard's wait were cut and counted as lost (lm1-rows-m3f.md, B0-32).
const GIT_TIMEOUT_MS = 200_000;
// B0-32's burst client: HTTP/1.1 on its own keep-alive pool, sized to the burst, as git itself pushes. On m3fr14 every
// burst push on the shared HTTP/2 client ended in a stream or socket reset with no answer, as B0-30 saw this orb's
// HTTP/2 connections to LAX reset under all their streams at once; the rest of the driver keeps `fetch` (HTTP/2), so
// its numbers stay comparable with earlier runs.
const BURST_PROTOCOL = "http/1.1 (node:https Agent, keepAlive, maxSockets = burst pushes)";
let burstAgent = null;
function postH1(url, body, headers, agent) {
  return new Promise((resolve, reject) => {
    const req = httpsRequest(url, { method: "POST", headers: { ...headers, "content-length": body.length }, agent, timeout: GIT_TIMEOUT_MS }, (res) => {
      const chunks = [];
      res.on("data", (c) => chunks.push(c));
      res.on("end", () => resolve({ status: res.statusCode, buf: Buffer.concat(chunks),
        headers: new Headers(Object.entries(res.headers).map(([k, v]) => [k, Array.isArray(v) ? v.join(", ") : String(v)])) }));
      res.on("error", reject);
    });
    req.on("timeout", () => req.destroy(Object.assign(new Error("timeout"), { code: "TIMEOUT" })));
    req.on("error", reject);
    req.end(body);
  });
}
async function git(path, tok, body, headers = {}, repo = R, agent = null) {
  const t = Date.now();
  if (agent) {
    try {
      const r = await postH1(`${EDGE}/${repo}.git/${path}`, body, { ...auth(tok), "user-agent": "git/2.56.0", ...failHeaders(), ...headers }, agent);
      return { status: r.status, text: r.buf.toString("latin1"), ms: Date.now() - t, headers: r.headers };
    } catch (e) { return { status: 0, text: String(e).slice(0, 80), ms: Date.now() - t, err: { name: e?.name, cause: e?.code ?? String(e?.cause ?? "").slice(0, 80) } }; }
  }
  try {
    const r = await fetch(`${EDGE}/${repo}.git/${path}`, { method: "POST", body, headers: { ...auth(tok), "user-agent": "git/2.56.0", ...failHeaders(), ...headers }, signal: AbortSignal.timeout(GIT_TIMEOUT_MS) });
    return { status: r.status, text: Buffer.from(await r.arrayBuffer()).toString("latin1"), ms: Date.now() - t, headers: r.headers };
  } catch (e) { return { status: 0, text: String(e).slice(0, 80), ms: Date.now() - t, err: { name: e?.name, cause: e?.cause?.code ?? String(e?.cause ?? "").slice(0, 80) } }; }
}
const lsRefs = (tok, prefix) => git("git-upload-pack", tok, Buffer.concat([pkt("command=ls-refs\n"), Buffer.from("0001"), pkt(`ref-prefix ${prefix}\n`), Buffer.from("0000")]),
  { "content-type": "application/x-git-upload-pack-request", "git-protocol": "version=2" });
const tip = async (ref) => (await lsRefs(admin, ref)).text.match(new RegExp(`([0-9a-f]{40}) ${ref}\\n`))?.[1] ?? null;
// A fresh repository's `main` (auto_init) was missing from ls-refs for seconds after its 201 on m23 (2 of 4 creates):
// wait up to 120 s and log the wait, so it is measured rather than a failed run.
const mainSince = Date.now();
let main = await tip("refs/heads/main");
while (!main && Date.now() - mainSince < 120_000) { await sleep(2_000); main = await tip("refs/heads/main"); }
if (!main) throw new Error("the repository has no main");
if (Date.now() - mainSince > 1_000) log(`main appeared in ls-refs after ${Date.now() - mainSince} ms`);

/** One push of `bytes` to `ref` from `from` (old tip or null); returns [result, ms, new tip]. */
const pushed = new Map(); // commit id -> ack time, for feed latency
const recentAgent = []; // [ok, ms] of the latest agent pushes, for B0-32's warm-up
async function push(tok, ref, from, path, bytes, extra = {}, agent = null) {
  const files = opt.workflow ? [[WF_PATH, WF_SRC]] : [];
  const { pack, commit } = onePack(path, bytes, from ?? main, `a1 ${ref}`, Math.floor(Date.now() / 1000), files);
  const r = await git("git-receive-pack", tok, receiveBody(ref, from, commit, pack), { "content-type": "application/x-git-receive-pack-request", ...extra }, R, agent);
  // A refusal keeps the first words of its body, so a 503's cause is counted (no secret is in a server's reply,
  // and every row passes `scan`).
  const res = r.status === 200 ? pushResult(r.text) : `http ${r.status} ${String(r.text ?? "").replace(/[0-9a-f]{12,}/g, "H").replace(/\d+/g, "N").slice(0, 160).trim()}`;
  if (res === "ok") pushed.set(commit, Date.now());
  notePushTiming(r.headers?.get?.("server-timing"), res);
  return [res, r.ms, res === "ok" ? commit : from, r.err, r.headers?.get?.("server-timing") ?? null];
}
// push-reads: a test edge's `server-timing` on every push (resolve, auth, install, gate, context, validate, admission,
// commit, stage, total), each stage's ms by name, plus how many commits ran the create probe (`probe`) and how many
// pushes staged before their first commit (`stagefirst`).
// receipt-rule: each commit's `/commit` sends (`send`), receipts missed while the member was busy and so neither
// resent at once nor reported (`busy`), `/outcome_by_request` asks while a send was open (`ask`), other missed
// receipts (`missed`), and `suspect` reports, one entry each.
const pushStages = {}; const pushMarks = { timed: 0, probed: 0, stagedFirst: 0, commits: 0, send: 0, busy: 0, ask: 0, missed: 0, suspect: 0 };
function notePushTiming(h, res) {
  if (!h) return;
  pushMarks.timed++;
  const seen = {};
  for (const m of h.matchAll(/([a-z_]+);dur=(\d+)/g)) {
    (pushStages[m[1]] ??= []).push(Number(m[2]));
    seen[m[1]] = (seen[m[1]] ?? 0) + 1;
  }
  pushMarks.probed += seen.probe ?? 0; pushMarks.stagedFirst += seen.stagefirst ? 1 : 0; pushMarks.commits += seen.commit ?? 0;
  for (const k of ["send", "busy", "ask", "missed", "suspect"]) pushMarks[k] += seen[k] ?? 0;
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
// L0-12's overlap probe: pairs of tasks that hand one Exclusive lease back and forth across the run (`probeLeases`).
const probers = plan.leases ? Array.from({ length: 2 * PROBE_PAIRS }, (_, k) => ({ i: plan.agents + k, task: null })) : [];
if (plan.leases || plan.side.taskEdit) {
  const want = plan.leases ? plan.agents + probers.length : Math.min(plan.agents, 100);
  const extra = plan.leases ? await seedRequesters(Math.max(0, Math.ceil(want / 100) - 3)) : [];
  const requesters = [null, "m3-member", "m3-admin", ...extra];
  for (const a of [...agents, ...probers].slice(0, want)) {
    const requester = requesters[Math.floor(a.i / 100)];
    if (requester === undefined) break;
    const r = await api("POST", `${V1}/tasks`, app, { kind: "external", session: `a1-${RUN}-${a.i}`, summary: "a1", ...(requester ? { requester } : {}) });
    if (r.status === 201) a.task = r.body.id;
  }
  const made = [...agents, ...probers].filter((a) => a.task).length;
  writeRow(OUT, { step: "tasks", made, want });
  log(`tasks ${made}/${want}`);
}
if (plan.leases) {
  // L0-12: a lease per agent, Exclusive on its own paths (one per task, a literal first segment), so the run holds
  // 500 Exclusive leases across the leader kills.
  for (const a of agents.filter((x) => x.task)) {
    const r = await api("POST", `${V1}/leases`, app, { paths: [`a1/${a.group}/${a.i}/**`], mode: "exclusive", ttl_seconds: 600 }, { "x-graftyard-agent-session": a.task });
    if (r.status === 201) a.lease = r.body?.lease?.id;
  }
  writeRow(OUT, { step: "leases", held: agents.filter((a) => a.lease).length, mode: "exclusive" });
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
// `--workflow` (B0-35's dispatch line): `main` takes the workflow first, so every branch and PR opened below has it.
if (opt.workflow) {
  const [res, , nu] = await push(admin, "refs/heads/main", main, "a1w/seed.txt", Buffer.from(`${RUN}\n`));
  if (res !== "ok") throw new Error(`workflow seed push: ${res}`);
  main = nu;
  writeRow(OUT, { step: "workflow seeded", main });
}
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
  for (const [k, pr] of openPrs.slice(0, plan.agents).entries()) { agents[k].ref = pr.ref; agents[k].tip = pr.tip; agents[k].prN = pr.number; }
}

// No learner catches up inside the window: before it starts, every learner has applied the leader's commit index as
// setup left it (up to 120 s), so a learner's join and catch-up, which raised a leader's wasm high-water from 28 to
// 73 MiB on graft-m3e (narrow-reads-3), falls in setup on every row. The wait and the leader's memory then are logged.
{
  const st = await statuses(CHAIN);
  const lead = st.find((s) => s.role === "leader") ?? await (async () => {
    const site = await (await import("../hosted/lib.mjs")).leaderOf(CHAIN).catch(() => undefined);
    return site === undefined ? undefined : (await statuses(CHAIN)).find((s) => s.site === site);
  })();
  const ls = (st.find((s) => s.config)?.config?.config?.normal?.learners ?? []).map((l) => ({ site: l.site, name: l.do_name }));
  if (lead && ls.length) {
    const since = Date.now();
    let behind = ls;
    while (behind.length && Date.now() - since < 120_000) {
      const at = await Promise.all(behind.map(async (l) => (await member(CHAIN, l.site, "/status", undefined, 15_000, l.name).catch(() => ({}))).v?.applied ?? 0));
      behind = behind.filter((_, i) => at[i] < lead.commit);
      if (behind.length) await sleep(2_000);
    }
    const mem = (await member(CHAIN, lead.site, "/stats?summary=1", undefined, 15_000).catch(() => ({}))).v?.wasmMemMb ?? null;
    writeRow(OUT, { step: "learners caught up", learners: ls.length, behind: behind.length, commit: lead.commit, waitMs: Date.now() - since, leaderWasmMb: mem });
    log(`learners caught up to ${lead.commit}: ${ls.length - behind.length}/${ls.length} in ${Date.now() - since} ms, leader wasm ${mem} MiB`);
  }
}
t0 = Date.now() + 5_000;
end = t0 + seconds * 1000;
// A SIGTERM (`tests/fanout/memguard.mjs`'s stop) ends the load window now, so the row is still written, as `stopped`.
let stopped = null;
process.on("SIGTERM", () => { stopped = new Date().toISOString(); end = Math.min(end, Date.now()); log(`SIGTERM: window ended at ${stopped}`); });

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
        const k = half(kind);
        f().then((code) => note(k, Date.now() - s, code)).catch((e) => note(k, Date.now() - s, `error ${String(e).slice(0, 30)}`))
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
// B0-34 round 26: every first-byte probe's clone-cache answer, and on a miss its `x-graft-compose` (composed, or stock
// with its reason) and whether the shard's spool left `clone/<net>/<key>/0` in R2 within a minute of the cancel.
const missRows = [];
const fillChecks = [];
/** The edge's CloneKey digest (crates/git/src/clone.rs `CloneKey::digest`) for the probe's fetch: one want, depth 1,
 *  no filter, caps `no-progress` and `ofs-delta`, no `include-tag` (so no tags digest). */
function cloneKeyHex(net, want) {
  const wants = createHash("sha256").update(Buffer.from(want, "hex")).digest();
  const be64 = (n) => { const b = Buffer.alloc(8); b.writeBigUInt64BE(BigInt(n)); return b; };
  const h = createHash("sha256").update("graftyard/clone/1\0").update(be64(net)).update(wants).update(be64(2))
    .update("\0none").update("\0");
  for (const c of ["no-progress", "ofs-delta"]) h.update(c).update("\0");
  return h.update(Buffer.alloc(32)).digest("hex");
}
/** A v2 fetch of `want` from `repo`: time to its first byte as `kind`. `firstByte` (B0-34's 5 GB network) asks depth
 *  1 with `ofs-delta` (the composable shape a `git clone --depth 1` sends) and cancels after the first byte; otherwise
 *  the whole answer is read. */
async function cloneTtfb(repo, want, kind, firstByte, tok = admin) {
  const s = Date.now();
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), 120_000);
  try {
    const r = await fetch(`${EDGE}/${repo}.git/git-upload-pack`, { method: "POST", signal: ctl.signal,
      headers: { ...auth(tok), "git-protocol": "version=2", "content-type": "application/x-git-upload-pack-request" },
      body: Buffer.concat([pkt("command=fetch\n"), Buffer.from("0001"), pkt("no-progress\n"), ...(firstByte ? [pkt("ofs-delta\n")] : []),
        pkt(`want ${want}\n`), ...(firstByte ? [pkt("deepen 1\n")] : []), pkt("done\n"), Buffer.from("0000")]) });
    const reader = r.body.getReader();
    await reader.read();
    const ms = Date.now() - s;
    note(kind, ms, String(r.status));
    if (firstByte && b034) {
      const cache = r.headers.get("x-graft-clone-cache"), compose = r.headers.get("x-graft-compose");
      note(`${kind}:${cache ?? "nocache"}`, ms, compose ?? String(r.status));
      if (cache === "miss") {
        const row = { at: new Date(s).toISOString(), ms, status: r.status, compose, want, key: cloneKeyHex(Number(CHAIN.split(":")[1]), want), filled: null };
        missRows.push(row);
        fillChecks.push((async () => {
          const by = Date.now() + 60_000;
          while (Date.now() < by) {
            await sleep(10_000);
            const found = (await r2list(`clone/${CHAIN.split(":")[1]}/${row.key}/0`)).filter((x) => x.keys?.length).map((x) => x.site);
            if (found.length) { row.filled = { sites: found, afterS: Math.round((Date.now() - s) / 1000) }; return; }
          }
          row.filled = false;
        })());
      }
    }
    if (firstByte) await reader.cancel();
    else while (!(await reader.read()).done);
    return String(r.status);
  } catch (e) { note(kind, Date.now() - s, `error ${String(e).slice(0, 30)}`); return "error"; }
  finally { clearTimeout(timer); }
}
// B0-35's dispatch line (`--workflow`; `decisions/DECISIONS.md`, "`pr-trigger`"): each PR head push's ack, and each
// `pull_request` run the admin runs route lists (newest 200, read every DISPATCH_POLL_MS from setup to DISPATCH_TAIL_MS
// past the window), first seen when. A run seen before the window is setup's (the PRs' `opened`).
const DISPATCH_POLL_MS = 1_000, DISPATCH_TAIL_MS = 90_000;
const dispatchAcks = [];
const dispatchRuns = new Map(); // run id -> { n, seenAt, trigger }
const dispatchPolls = {};
const pollRuns = async () => {
  const r = await api("GET", `/api/graftyard/v1/repos/${R}/actions/runs`, admin);
  dispatchPolls[r.status] = (dispatchPolls[r.status] ?? 0) + 1;
  for (const x of r.body?.runs ?? []) {
    const n = x.event === "pull_request" ? Number(String(x.head_ref).match(/^refs\/pull\/(\d+)\/merge$/)?.[1]) : NaN;
    if (!dispatchRuns.has(x.id)) dispatchRuns.set(x.id, { n, seenAt: Date.now(), trigger: x.trigger, status: x.status });
  }
};
const jobs = [
  ...(opt.workflow ? [(async () => {
    while (Date.now() < end + DISPATCH_TAIL_MS) {
      const s = Date.now();
      await pollRuns().catch(() => { dispatchPolls.error = (dispatchPolls.error ?? 0) + 1; });
      await sleep(Math.max(0, DISPATCH_POLL_MS - (Date.now() - s)));
    }
  })()] : []),
  // Each agent pushes every pushEvery s, its start spread over the first interval.
  ...agents.map((a) => (async () => {
    await sleep(Math.max(0, t0 - Date.now()) + Math.random() * plan.pushEvery * 1000);
    while (Date.now() < end) {
      const s = Date.now();
      const kill = plan.killEvery && ++pushN % Math.max(1, Math.round(plan.killEvery * plan.agents / plan.pushEvery)) === 0;
      // The leader as the kill push leaves, read beside it so the push is not delayed.
      const seenAt = kill ? leaderSeen() : null;
      const k = half(kill ? "push:killed" : "push:agent");
      const [res, ms, nu] = await push(app, a.ref, a.tip, `a1/${a.group}/${a.i}/f.txt`, randomBytes(size()), kill ? { "x-graft-kill": "after_quorum" } : {});
      if (opt.workflow && res === "ok" && a.prN) dispatchAcks.push({ n: a.prN, at: Date.now() });
      a.tip = nu;
      if (kill) { kills.push(s); killRows.push(killRow(s, ms, res, seenAt)); }
      note(k, ms, res);
      if (!kill) { recentAgent.push([res === "ok", ms]); if (recentAgent.length > 200) recentAgent.splice(0, 100); }
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
  // Humans share 20 refs, so two pushes to one ref can race and the later one is `Stale`, as git's own lease is. B0-29's
  // gate is zero `Stale` on distinct refs: a `stale` whose ref had another human push in flight is `stale:race`.
  every("push:human", plan.human / 2, async () => {
    const k = Math.floor(Math.random() * 20);
    const ref = `refs/heads/a1h-${RUN}-${k}`;
    humanInFlight[k] = (humanInFlight[k] ?? 0) + 1;
    let raced = humanInFlight[k] > 1;
    try {
      const [res] = await push(admin, ref, await tip(ref), `a1h/${k}.txt`, randomBytes(20_000));
      raced ||= humanInFlight[k] > 1;
      return res === "stale" && raced ? "stale:race" : res;
    } finally { humanInFlight[k]--; }
  }, 5),
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
    const k = half("merge:only");
    const m = await api("PUT", `/api/v3/repos/${R}/pulls/${pr.number}/merge`, admin, { merge_method: "merge" });
    note(k, Date.now() - s, m.status === 200 ? "200" : `${m.status} ${String(m.body?.message ?? "").replace(/\d+/g, "N").slice(0, 70)}`);
    // Where the merge's time went: the edge's phases (`server-timing`) and the shard's own (`x-graft-shard-timing`).
    mergeSteps.push({ ms: Date.now() - s, status: m.status, timing: m.headers["server-timing"] ?? null, shard: m.headers["x-graft-shard-timing"] ?? null });
    if (m.status === 200) main = (await tip("refs/heads/main")) ?? main;
    return st(m);
  }, 3),
  every("read:ref", plan.refReads, async () => st(await lsRefs(admin, "refs/heads/"))),
  every("read:api", plan.apiReads, async () => stages(Math.random() < 0.5
    ? await api("GET", `/api/v3/repos/${R}/issues?per_page=30`, admin)
    : await api("POST", "/api/graphql", admin, { query: `query { repository(owner: "m3org", name: "${opt.repo}") { pullRequests(last: 10) { nodes { number isDraft${plan.openPrs ? " mergeable" : ""} } } } }` }))),
  every("read:clone", plan.clones, () => cloneTtfb(R, main, "read:cloneTtfb", !!b034), 20),
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
// B0-32's burst "rides a hot group": the journey 9 load runs until the leader reports `hot` and the network's shards
// are warm, then the burst fires and the row runs `seconds - burst.at` past it. Warm is the last WARM_PUSHES agent
// pushes all acknowledged within 5 s, which a cold pool shard's `warming` refusals and 15 s home waits fail (m3fr1:
// a 5-minute-old group's pool shards rehydrated during the burst).
const WARM_PUSHES = 30, WARM_MAX_S = 900;
if (plan.burst) jobs.push((async () => {
  await sleep(Math.max(0, t0 + plan.burst.at * 1000 - Date.now()));
  const w0 = Date.now();
  // The load runs while `Date.now() < end`: hold `end` past the warm-up's longest wait, or the load stops at
  // `seconds` and the group cools while the warm-up waits (m3fr2: load ended at 180 s, `hot` false from then on).
  end = w0 + (WARM_MAX_S + seconds) * 1000;
  let hot = false, warm = false, polls = 0;
  while (Date.now() - w0 < WARM_MAX_S * 1000) {
    const l = (await statuses(CHAIN)).filter((x) => x.role === "leader").sort((a, b) => b.term - a.term)[0];
    hot = !!l?.admission?.hot;
    warm = recentAgent.length >= WARM_PUSHES && recentAgent.slice(-WARM_PUSHES).every(([ok, ms]) => ok && ms <= 5_000);
    polls++;
    if (hot && warm) break;
    await sleep(2_000);
  }
  writeRow(OUT, { step: "burst warm-up", seconds: Math.round((Date.now() - w0) / 1000) + plan.burst.at, hot, warm, polls,
    lastAgentMs: recentAgent.slice(-WARM_PUSHES).map(([, ms]) => ms) });
  log(`burst warm-up: hot ${hot}, warm ${warm} after ${Math.round((Date.now() - w0) / 1000) + plan.burst.at} s`);
  end = Date.now() + (seconds - plan.burst.at) * 1000;
  burstAgent = new HttpsAgent({ keepAlive: true, maxSockets: plan.burst.pushes });
  const s = Date.now();
  // Client-side record of every burst push, so one with no edge event still shows when it was sent and how it ended.
  const sent = [];
  const r = await Promise.all(Array.from({ length: plan.burst.pushes }, (_, k) => sleep((k * 1000) / plan.burst.pushes).then(async () => {
    const a = agents[k % agents.length];
    const at = Date.now();
    const [res, ms, , err, timing] = await push(app, `${a.ref}-b`, null, `a1/${a.group}/${a.i}/burst.txt`, randomBytes(plan.burst.bytes), {}, burstAgent);
    note("push:burst", ms, res);
    sent.push({ k, sentAt: new Date(at).toISOString(), ms, res: String(res).slice(0, 120), timing, ...(err ? { err } : {}) });
    return [res, Date.now() - s];
  })));
  burstAgent.destroy();
  writeRow(OUT, { step: "burst", protocol: BURST_PROTOCOL, hot, warm, pushes: plan.burst.pushes, within5s: r.filter(([x, t]) => x === "ok" && t <= 5_000).length, results: r.reduce((m, [x]) => ((m[x] = (m[x] ?? 0) + 1), m), {}),
    unacked: sent.filter((x) => x.res !== "ok" && !String(x.res).startsWith("full")) });
  // Every burst push's stages, in send order, for where each one's time went.
  writeRow(OUT, { step: "burst timing", pushes: sent.sort((x, y) => x.k - y.k).map(({ k, ms, res, timing }) => ({ k, ms, res: String(res).slice(0, 40), timing })) });
})());
// L0-12's overlap probe. Pair k's holder takes `a1p/<RUN>/<k>/**` Exclusive for PROBE_TTL_S and never renews it;
// from 2 s before its end the other task asks for the same scope every PROBE_EVERY_MS until it gets a 201, which is
// one sample: `overlapMs` of the two leases' leader-stamped grants. Then the roles swap. Pair starts are spread over
// the TTL, so samples fall on both sides of the leader kills.
const overlaps = []; // [overlap ms, asks, status counts]
if (plan.leases) for (let k = 0; k < PROBE_PAIRS; k++) {
  const pair = [probers[2 * k], probers[2 * k + 1]];
  if (!pair[0]?.task || !pair[1]?.task) continue;
  jobs.push((async () => {
    const scope = { paths: [`a1p/${RUN}/${k}/**`], mode: "exclusive", ttl_seconds: PROBE_TTL_S };
    const ask = (p) => api("POST", `${V1}/leases`, app, scope, { "x-graftyard-agent-session": p.task });
    await sleep(Math.max(0, t0 - Date.now()) + (k * PROBE_TTL_S * 1000) / PROBE_PAIRS);
    let held = null;
    for (let t = 0; t < 20 && !held && Date.now() < end; t++) {
      const r = await ask(pair[0]);
      if (r.status === 201) held = r.body?.lease?.id; else await sleep(1000);
    }
    let h = 0;
    while (held && Date.now() < end) {
      await sleep(Math.max(0, ulidMs(held) + PROBE_TTL_S * 1000 - 2000 - Date.now()));
      const codes = {};
      let got = null;
      for (let n = 0; !got && Date.now() < end; n++) {
        const s = Date.now();
        const r = await ask(pair[1 - h]);
        codes[r.status] = (codes[r.status] ?? 0) + 1;
        if (r.status === 201) got = r.body?.lease?.id;
        else await sleep(Math.max(0, PROBE_EVERY_MS - (Date.now() - s)));
      }
      if (!got) break;
      overlaps.push([overlapMs(held, PROBE_TTL_S, got), Object.values(codes).reduce((a, b) => a + b, 0), codes]);
      held = got;
      h = 1 - h;
    }
  })());
}
// Member clock samples for L0-12: each member's `/status` `now` against the midpoint of its round trip, every 15 s.
const clockSamples = [];
let clockRound = 0;
// L0-12's clock line: `clockSpread` in clock.mjs.
// The driver's clock against the edge's `Date` (clock.mjs `driverOffset`), before the window and after it: each
// member offset is judged against Cloudflare's clock, not this orb's (m3fr3: the orb read 1.5 to 2.0 s behind).
const driverClock = {};
if (plan.leases) jobs.push((async () => {
  driverClock.before = await driverOffset(`${EDGE}/api/v3/meta`);
  await sleep(Math.max(0, t0 - Date.now()));
  while (Date.now() < end) {
    clockRound++;
    await Promise.all([0, 1, 2].map(async (site) => {
      const s = Date.now();
      try {
        // The site's current voter object: by site alone a replaced voter's first-generation object answers.
        // `?clock=1` (leader-clock-check): the member's clock before and after one awaited macrotask, and its last
        // appended entry's `at` lag at send and one macrotask later; older builds answer plain `/status`.
        const v = (await member(CHAIN, site, "/status?clock=1", undefined, 5_000, voterAt(CHAIN, site))).v;
        const e = Date.now();
        if (v?.now) clockSamples.push({ round: clockRound, site, offset: v.now - (s + e) / 2, rtt: e - s, term: v.term, role: v.role,
          before: v.clockBefore != null ? v.clockBefore - (s + e) / 2 : null, tick: v.clockAfter != null ? v.clockAfter - v.clockBefore : null,
          lease: v.leaseClock ?? null });
      } catch { /* a member mid-kill */ }
    }));
    await sleep(15_000);
  }
  driverClock.after = await driverOffset(`${EDGE}/api/v3/meta`);
})());
if (opt["kill-shard-at"]) jobs.push((async () => {
  await sleep(Math.max(0, t0 + Number(opt["kill-shard-at"]) * 1000 - Date.now()));
  const { bedGet } = await import("../hosted/lib.mjs");
  const id = CHAIN.split(":")[1];
  const r = await bedGet(`/_bed/kill?repo=${id}`);
  writeRow(OUT, { step: "shard kill", status: r.status, at: Date.now() - t0 });
})());

// B0-34 as written. Phase marks go in the row's file; every kind measured in a phase is suffixed with it.
function b034Phase() {
  const past = (at) => at !== undefined && at !== null && Date.now() >= t0 + Number(at) * 1000;
  return past(b034.hostileAt) ? "@hostile" : past(b034.secondAt) ? "@second" : past(opt["kill-shard-at"]) ? "@kill" : "@j9";
}
const kTimeline = []; // [s since t0, chain, k]
if (b034) {
  const { execFileSync: ex } = await import("node:child_process");
  // The tip: the next real first-parent commit, fast-forwarded onto main under the admin's PAT from the bare clone.
  const g = new (await import("../hosted/lib.mjs")).Git(R).as(admin);
  // From the commit after main's place in that history: the repository keeps main between rounds, so from index 0
  // every move was a non-fast-forward (round 26: 60 of 60 refused in the j9 and kill phases).
  let next = Math.max(0, b034.future.indexOf(main) + 1);
  writeRow(OUT, { step: "b034 tip start", main, next, of: b034.future.length });
  jobs.push(every("tip:move", plan.tipMovesPerMin / 60, async () => {
    const c = b034.future[next];
    if (!c) return "no more history";
    next++;
    const r = g.run(["push", "-q", g.url, `${c}:refs/heads/main`], b034.bare);
    if (r.ok) { main = c; return "ok"; }
    return `refused ${r.out.split("\n").filter(Boolean).slice(-1)[0]?.replace(/[0-9a-f]{12,}/g, "H").slice(0, 80) ?? ""}`;
  }, 1));
  void ex;
  // A hot network of another owner: pushes of one new blob on its own branches and depth-1 clones read whole.
  const hot = (n, from, tag) => {
    const tips = {};
    const tok = tokens[n.who];
    const pushPerS = b034.hotPushPerS ?? 1, clonePerS = b034.hotClonePerS ?? 3;
    const at = async () => { await sleep(Math.max(0, t0 + from * 1000 - Date.now())); };
    return [
      (async () => { await at(); await every(`hot:push${tag}`, pushPerS, async () => {
        const k = Math.floor(Math.random() * 20), ref = `refs/heads/b034-${RUN}-${k}`;
        const base = tips[ref] ?? (await git("git-upload-pack", tok, Buffer.concat([pkt("command=ls-refs\n"), Buffer.from("0001"), pkt("ref-prefix refs/heads/main\n"), Buffer.from("0000")]),
          { "content-type": "application/x-git-upload-pack-request", "git-protocol": "version=2" }, n.repo)).text.match(/([0-9a-f]{40}) refs\/heads\/main\n/)?.[1];
        const { pack, commit } = onePack(`b034/${k}.txt`, randomBytes(20_000), tips[ref] ?? base, `b034 ${ref}`, Math.floor(Date.now() / 1000));
        const r = await git("git-receive-pack", tok, receiveBody(ref, tips[ref] ?? null, commit, pack), { "content-type": "application/x-git-receive-pack-request" }, n.repo);
        const res = r.status === 200 ? pushResult(r.text) : `http ${r.status}`;
        if (res === "ok") tips[ref] = commit;
        return res;
      }, 8); })(),
      (async () => { await at(); await every(`hot:clone${tag}`, clonePerS, async () => {
        const t = (await git("git-upload-pack", tok, Buffer.concat([pkt("command=ls-refs\n"), Buffer.from("0001"), pkt("ref-prefix refs/heads/main\n"), Buffer.from("0000")]),
          { "content-type": "application/x-git-upload-pack-request", "git-protocol": "version=2" }, n.repo)).text.match(/([0-9a-f]{40}) refs\/heads\/main\n/)?.[1];
        return t ? cloneTtfb(n.repo, t, `hot:cloneTtfb${tag}`, false, tok) : "no main";
      }, 8); })(),
    ];
  };
  if (b034.second) jobs.push(...hot(b034.second, b034.secondAt, ":second"));
  for (const [i, h] of (b034.hostile ?? []).entries()) jobs.push(...hot(h, b034.hostileAt, `:hostile${i + 1}`));
  // k of every network, every 30 s, from each group's leader (`ranges`, as tests/fanout/k.mjs reads it).
  const nets = [CHAIN, b034.second?.chain, ...(b034.hostile ?? []).map((h) => h.chain)].filter(Boolean);
  jobs.push((async () => {
    await sleep(Math.max(0, t0 - Date.now()));
    while (Date.now() < end) {
      for (const c of nets) {
        try {
          const l = (await statuses(c)).filter((x) => x.role === "leader").sort((a, b) => b.term - a.term)[0];
          const v = l ? await member(c, l.site, "/read", { q: { ranges: true } }, 15_000) : null;
          kTimeline.push([Math.round((Date.now() - t0) / 1000), c, v?.v?.value?.fanout ?? null]);
        } catch { kTimeline.push([Math.round((Date.now() - t0) / 1000), c, null]); }
      }
      await sleep(30_000);
    }
  })());
}

// The feed: pollers straight to followers and learners, as the edge sends them, each watching one agent group.
const seen = []; // [poller, ref, ms after its push's ack, seen at]
const feedAnswers = {}; // "site:role:member" -> { admit: count }, every member answer the pollers got
// L0-12 judges feed p99 outside leader kills: each kill push's send time; an event seen within KILL_WINDOW_MS after one
// is inside a kill (an election, ELECTION_AFTER plus a campaign, about 10 s [inference]).
const kills = [];
const KILL_WINDOW_MS = 15_000;
// Each kill as its own `kill` row (m3fr6 kept only the count): when it was sent, the leader the driver saw then (site,
// term, boot), and after the push answered, the leader then and whether that site's member booted again, which is the
// site the kill reset. `boot` is drawn when a member object starts, so a changed boot is a reset member.
const killRows = [];
const leaderSeen = async () => {
  const st = await statuses(CHAIN).catch(() => []);
  const l = st.filter((x) => x.role === "leader").sort((a, b) => b.term - a.term)[0];
  return { at: Date.now(), leader: l?.site ?? null, term: l?.term ?? null, boots: Object.fromEntries(st.map((x) => [x.site, x.boot ?? null])) };
};
const killRow = async (sent, ms, res, seenAt) => {
  const before = await seenAt;
  // The killed member resets after its quorum; read again once it has had KILL_SETTLE_MS to come back.
  await sleep(KILL_SETTLE_MS);
  const after = await leaderSeen();
  const reset = Object.keys(after.boots).filter((s) => before.boots[s] && after.boots[s] && before.boots[s] !== after.boots[s]).map(Number);
  const r = { step: "kill", sent, ms, res, before: { at: before.at, leader: before.leader, term: before.term },
    after: { at: after.at, leader: after.leader, term: after.term }, reset, killed: reset.includes(before.leader) ? before.leader : (reset[0] ?? null) };
  writeRow(OUT, r);
  log(`kill ${new Date(sent).toISOString()}: leader ${before.leader} term ${before.term} -> ${after.leader} term ${after.term}, reset ${JSON.stringify(reset)}`);
  return r;
};
const KILL_SETTLE_MS = 5_000;
if (plan.pollers) {
  // The members a poll may go to, as the edge's `feed_members` lists them (workers/edge/src/agents.rs): learners
  // first, then the voters that do not lead, re-read every 60 s and at once when a voter answers `notServing` (it
  // leads now).
  let members = [], membersAt = 0;
  const readMembers = async () => {
    const st = await statuses(CHAIN);
    const cfg = st.find((s) => s.config)?.config?.config?.normal;
    const learners = (cfg?.learners ?? []).map((l) => ({ site: l.site, name: l.do_name, learner: true }));
    const voters = (cfg?.voters ?? []).filter((v) => !st.some((s) => s.site === v.site && s.role === "leader"))
      .map((v) => ({ site: v.site, name: v.do_name, learner: false }));
    members = [...learners, ...voters];
    membersAt = Date.now();
    return { learners: learners.length, followers: voters.length };
  };
  writeRow(OUT, { step: "feed members", ...(await readMembers()) });
  // One poll as the edge's `activity` routes it: members in order (this poller's learner first, so 500 pollers
  // spread over the learners as the edge's isolates' members do), up to three that answer; the first `hold` is the
  // answer; a full member answers 429; a `gone` sends the poll on, and is the answer only if every one tried answered
  // it (`graftyard_git::feed::route`); if every other one refused as lagging and the least lagged is a learner, that
  // learner holds the poll with `lagging_ok`; otherwise 429. `learner_answers` is set on followers once a learner
  // answered `notServing`, as the edge sets it.
  const poll = async (k, cursor) => {
    if (Date.now() - membersAt > 60_000) await readMembers();
    const ls = members.filter((m) => m.learner), vs = members.filter((m) => !m.learner);
    const order = [...ls.slice(k % Math.max(1, ls.length)), ...ls.slice(0, k % Math.max(1, ls.length)), ...vs];
    const body = (extra) => ({ cursor, wait_s: 25, paths: [`a1/${k % 50}/**`], token: `a1-${RUN}-${k}`, ip: `a1ip-${k}`, ...extra });
    const answers = [], tried = [];
    let learnerAnswered = false, gone = null;
    for (const m of order) {
      if (answers.length >= 3) break;
      let r;
      try { r = await member(CHAIN, m.site, "/feed", body({ learner_answers: learnerAnswered && !m.learner }), 40_000, m.name); } catch { continue; }
      const v = r.v ?? {};
      tried.push(v.admit ?? `status ${r.status}`);
      // Each member's answers by kind, so a run shows which member sent polls back `gone` or `lagging`.
      const by = (feedAnswers[`${m.site}:${m.learner ? "learner" : "voter"}:${m.name}`] ??= {});
      by[v.admit ?? `status ${r.status}`] = (by[v.admit ?? `status ${r.status}`] ?? 0) + 1;
      if (r.status !== 200 || !v.admit) continue;
      if (v.admit === "hold") return { v, m, tried };
      if (v.admit === "tooMany") return { v, m, tried };
      if (v.admit === "gone") { gone ??= { v, m }; answers.push({ a: "gone", m }); }
      else if (v.admit === "full") answers.push({ a: "full", m, lag: 0 });
      else if (v.admit === "lagging") answers.push({ a: "lagging", m, lag: v.lagMs ?? Number.MAX_SAFE_INTEGER });
      else if (v.admit === "notServing" && m.learner) learnerAnswered = true;
      else if (v.admit === "notServing") membersAt = 0;
    }
    if (answers.some((x) => x.a === "full")) return { v: { admit: "429" }, tried };
    const others = answers.filter((x) => x.a !== "gone");
    if (gone && answers.length > 0 && others.length === 0) return { v: gone.v, m: gone.m, tried };
    const lag = others.filter((x) => x.a === "lagging");
    const least = lag.sort((a, b) => a.lag - b.lag)[0];
    if (least && lag.length === others.length && least.m.learner) {
      try {
        const r = await member(CHAIN, least.m.site, "/feed", body({ learner_answers: false, lagging_ok: true }), 40_000, least.m.name);
        if (r.v?.admit === "hold") return { v: r.v, m: least.m, tried: [...tried, "laggingOk"] };
      } catch { /* 429 below */ }
    }
    return { v: { admit: "429" }, tried };
  };
  for (let k = 0; k < plan.pollers; k++) {
    jobs.push((async () => {
      await sleep(Math.max(0, t0 - Date.now()) + (k % 500) * 4);
      let cursor = null, misses = {};
      while (Date.now() < end) {
        const s = Date.now();
        try {
          const { v, m, tried } = await poll(k, cursor);
          note("feed", Date.now() - s, v.admit);
          // Polls since this poller last held: what an event seen late waited on. Each 429 counts the members tried.
          const missed = misses;
          if (v.admit === "hold") misses = {};
          else for (const t of ["429", ...tried]) misses[t] = (misses[t] ?? 0) + 1;
          // A 410 names the oldest cursor the member serves; a client follows it, as from `oldest_cursor`.
          if (v.cursor) cursor = v.cursor;
          else if (v.admit === "gone" && v.oldest) cursor = v.oldest;
          for (const e of v.events ?? []) {
            const p = e[2]?.AgentPush;
            const head = p ? (typeof p.head === "string" ? p.head : Buffer.from(p.head).toString("hex")) : null;
            if (head && pushed.has(head)) seen.push([k, head, Date.now() - pushed.get(head), Date.now(), m?.site, missed]);
          }
          // A 429 carries Retry-After FEED_BATCH_ALL (10 s); a client that honours it waits that long.
          if (v.admit === "429") await sleep(10_000);
        } catch (e) { note("feed", Date.now() - s, "error"); misses.error = (misses.error ?? 0) + 1; await sleep(1000); }
      }
    })());
  }
}

// B0-35's and B0-31's instruments: each site's shard of the network, its test-merge budget's counts (null before any merge).
const budgets = () => Promise.all([0, 1, 2].map(async (s) => {
  try { return JSON.parse((await bedGet(`/_bed/shard_status?repo=${CHAIN.split(":")[1]}&shard=${s}`)).body).testMerges ?? null; } catch { return null; }
}));
const budgetBefore = await budgets();
// The shard object keeps its counts in memory, and hosted it restarted 10 to 30 s after the load stopped
// (`tests/am1/results/merge-budget-2.md`), so the run samples them every 15 s and keeps the latest of each site.
const budgetSeen = [...budgetBefore];
/** Each site's counts after the window: the read now, or, for a site whose object restarted since (a later `since`
 *  with fewer merges, or none), the last sample of the run. */
function latestBudget(now, seen) {
  return now.map((b, s) => (!b || (seen[s] && b.since !== seen[s].since && b.computed < seen[s].computed)) ? seen[s] ?? b : b);
}
const leaderBefore = await statuses(CHAIN);
// One `/status` round can miss the leader (m25cap500, 131326: "leader site undefined", and the row lost its leader
// stats), so `leaderOf` asks again, up to 20 s.
const leader = leaderBefore.find((s) => s.role === "leader")?.site
  ?? await (await import("../hosted/lib.mjs")).leaderOf(CHAIN).catch(() => undefined);
if (leader !== undefined) await member(CHAIN, leader, "/stats?reset=1", undefined, 15_000);
// `--first-for`: round 5's concurrent re-sends, on purpose. Every member gets it, so a leader elected mid-run has it too.
if (opt["first-for"]) for (const s of [0, 1, 2]) {
  const v = (await member(CHAIN, s, "/bench_seed", { prsyncFirstFor: Number(opt["first-for"]) }, 15_000).catch((e) => ({ v: String(e) }))).v;
  log(`site ${s} prsyncFirstFor ${JSON.stringify(v)}`);
}
log(`${row} run ${RUN}: ${plan.agents} agents, ${seconds} s from ${new Date(t0).toISOString()}, leader site ${leader}`);
// The leader's `/stats` 20 s before the end too: B0-29's end-of-row read raised and the isolate reset (round 5), and the
// commit split is in memory only. The row uses the end read when it has samples, else this one.
let lateStats = null;
// Only with `?summary=1` (percentiles, no raw arrays): the raw read of a loaded leader reset its wasm instance and
// moved the leader (B0-29, 131261, 23:44:20Z), so the late read runs only on a release that answers summaries.
// B0-31 gates commit p99 with buckets up on its own: the leader's stats at `placeFailAt`, then reset, so the end read
// holds only the placement-failing half.
let upStats = null;
if (leader !== undefined && plan.placeFailAt !== undefined) jobs.push((async () => {
  await sleep(Math.max(0, t0 + plan.placeFailAt * 1000 - Date.now()));
  upStats = (await member(CHAIN, leader, "/stats?summary=1&reset=1", undefined, 15_000).catch(() => ({}))).v ?? null;
  log(`buckets-up half: commit ${JSON.stringify(upStats?.commitMs ?? null)}`);
})());
if (leader !== undefined) jobs.push((async () => {
  // `end` can move later (B0-32's warm-up), so wait on it as it stands.
  while (Date.now() < end - 20_000) await sleep(Math.min(5_000, end - 20_000 - Date.now()));
  lateStats = (await member(CHAIN, leader, "/stats?summary=1", undefined, 15_000).catch(() => ({}))).v ?? null;
})());
// Round 14: the leader's admission every 10 s (capacity, queued, agent queue and bytes, pressure), from the summary.
const admissionSeen = [];
if (leader !== undefined) jobs.push((async () => {
  await sleep(Math.max(0, t0 - Date.now()));
  while (Date.now() < end) {
    const v = (await member(CHAIN, leader, "/stats?summary=1", undefined, 10_000).catch(() => ({}))).v;
    if (v?.admission) admissionSeen.push({ s: Math.round((Date.now() - t0) / 1000), ...v.admission, wasmMemMb: v.wasmMemMb });
    await sleep(10_000);
  }
})());
jobs.push((async () => {
  while (Date.now() < end) {
    await sleep(15_000);
    (await budgets()).forEach((b, s) => { if (b && (!budgetSeen[s] || b.since === budgetSeen[s].since || b.computed >= budgetSeen[s].computed)) budgetSeen[s] = b; });
  }
})());
await Promise.all(jobs);
// The last first-byte misses' R2 checks run up to a minute past the window.
await Promise.all(fillChecks);
const killLog = await Promise.all(killRows);
if (opt["first-for"]) for (const s of [0, 1, 2]) await member(CHAIN, s, "/bench_seed", { prsyncFirstFor: 0 }, 15_000).catch(() => {});
// Give the last pushes' first attempts time to reach the shard.
const budgetAfter = latestBudget((await sleep(15_000), await budgets()), budgetSeen);
const after = await statuses(CHAIN);
// A leader mid-reset answers `/stats` with no body; the row still writes its own counts.
// Always `?summary=1`: a loaded leader's raw arrays (megabytes) rebuilt its wasm instance (round 6, B0-29).
const endStats = (leader !== undefined ? (await member(CHAIN, leader, "/stats?summary=1", undefined, 15_000).catch(() => ({}))).v : null) ?? {};
// The leader's failed calls by peer (B0-32 m3fr2: 1,032 `failedCalls`, no target named): the full `/stats` `peers`
// (per peer ok, sent, failed and late by cause, newest errors), read once after the window so a raw read's reset
// cannot fall inside it, and before teardown, so the leader object has not restarted yet. Written as its own row.
if (leader !== undefined) {
  const full = (await member(CHAIN, leader, "/stats", undefined, 30_000).catch((e) => ({ v: { error: String(e).slice(0, 120) } }))).v ?? {};
  writeRow(OUT, { step: "leader peers", site: leader, failedCalls: full.failedCalls, peers: full.peers ?? null, error: full.error });
  log(`leader peers: failedCalls ${full.failedCalls} ${JSON.stringify(Object.fromEntries(Object.entries(full.peers ?? {}).map(([p, c]) => [p, { ok: c.ok, failed: c.failed, late: c.late, lateFailed: c.lateFailed }]))).slice(0, 600)}`);
}
const lstats = endStats.commitMs?.n ? endStats : (lateStats ?? endStats);
function driverMs() {
  const xs = [driverClock.before, driverClock.after].filter(Boolean).map((d) => d.offsetMs);
  return xs.length ? Math.round(xs.reduce((a, b) => a + b, 0) / xs.length) : null;
}
const statsFrom = endStats.commitMs?.n ? "end" : lateStats ? "late" : "none";
// A sample array, or `/stats?summary=1`'s `{n, p50, p99}` of one.
const ps = (x) => (Array.isArray(x) || x == null ? { n: x?.length, p50: pct(x, 0.5), p99: pct(x, 0.99) } : { n: x.n, p50: x.p50, p99: x.p99 });
const splitOf = (st) => Object.fromEntries([["queue", st.queueMs], ["queue:slot", st.queueSlotMs], ["queue:post", st.queuePostMs],
  ["round", st.roundMs], ["confirm", st.confirmMs], ["apply", st.applyMs],
  // read-ahead's flip (B0-31): the leader's cycle, append to append, and the frozen batches held at each append.
  ["cycle", st.cycleMs], ["held", st.heldAtAppend],
  ...Object.entries(st.appendMs ?? {}).map(([k, xs]) => [`append:${k}`, xs]),
  ...Object.entries(st.depsMs ?? {}).map(([k, xs]) => [`deps:${k}`, xs])]
  .map(([k, xs]) => [k, ps(xs)]));
// A stopped window's rates are over the seconds it ran.
const ran = stopped ? Math.max(1, (end - t0) / 1000) : seconds;
const sum = Object.fromEntries(Object.entries(lat).map(([k, xs]) => [k, { n: xs.length, perS: +(xs.length / ran).toFixed(2), p50: pct(xs, 0.5), p99: pct(xs, 0.99), codes: codes[k] }]));
const feedLat = seen.map((x) => x[2]);
const v = {
  row, run: RUN, seconds, ...(stopped ? { stopped, ranSeconds: +ran.toFixed(1) } : {}), agents: plan.agents, offered: offered(plan), byKind: sum,
  ...(killLog.length ? { kills: killLog.map(({ sent, before, after, reset, killed, res }) => ({ sent, leader: before.leader, term: before.term, killed, reset, after: after.leader, termAfter: after.term, res })) } : {}),
  feed: { events: seen.length, p50: pct(feedLat, 0.5), p99: pct(feedLat, 0.99), max: feedLat.length ? Math.max(...feedLat) : null, byMember: feedAnswers,
    ...(kills.length ? { outsideKills: (() => { const xs = seen.filter(([, , ms, at]) => !kills.some((k) => at - ms < k + KILL_WINDOW_MS && at >= k)).map((x) => x[2]);
      return { kills: kills.length, events: xs.length, p50: pct(xs, 0.5), p99: pct(xs, 0.99) }; })() } : {}),
    // The slowest 1% outside kills: which member served them and what the poller's misses were before the hold.
    slow: (() => { const out = seen.filter(([, , ms, at]) => !kills.some((k) => at - ms < k + KILL_WINDOW_MS && at >= k));
      const cut = pct(out.map((x) => x[2]), 0.99) ?? Infinity;
      return out.filter((x) => x[2] >= cut).slice(0, 200).map(([k, , ms, at, site, miss]) => ({ k, ms, at, site, miss })); })() },
  leader: { statsFrom, before: leader, after: after.find((s) => s.role === "leader")?.site, terms: after.map((s) => [s.site, s.role, s.term]),
    commitMs: ps(lstats.commitMs),
    // B0-31: `commitMs` above is then the placement-failing half alone, and this the buckets-up half.
    ...(plan.placeFailAt !== undefined ? { commitMsByHalf: { up: ps(upStats?.commitMs), fail: ps(lstats.commitMs) } } : {}),
    // Round 5's commit split: admission to append (`queueMs`, which holds the entry's dep reads), the dep reads by
    // chain kind (`depsMs`), append to apply (`roundMs`: storage confirm and quorum), and the confirm alone.
    // Round 10 adds each follower's AppendEntries call time (`append:<site>`) and append to apply (`apply`), so the
    // round splits into the near follower's leg and the leader's own confirm.
    // `queue:slot` waits for the entries ahead; `queue:post` is the command's own entry's cut, dep reads and decide.
    split: splitOf(lstats),
    // B0-31: the same split for the buckets-up half, read at `placeFailAt` before the reset, to trace that half's tail.
    ...(plan.placeFailAt !== undefined ? { splitByHalf: { up: splitOf(upStats ?? {}), fail: splitOf(lstats) } } : {}),
    // Round 12's confirm split by transaction size: the gate wait, the turn queue, and storage (`sync - turn`).
    confirmSplit: lstats.confirmSplit ?? null,
    // Round 14: admitted commands' priced bytes by class (what moves admission's `cmdBytes`), and the statement mix of
    // the leader's newest transactions of 100 KB or more.
    admittedBytes: lstats.admittedBytes ?? null, bigTx: lstats.bigTx ?? null,
    depFails: lstats.depFails, depHedges: lstats.depHedges ?? null,
    // `Events` entries the leader appended and the commands in them (commands per entry, rounds 9 and 10).
    entries: lstats.entries, commands: lstats.commands,
    reqPerS: lstats.inboundMs ? +(Object.values(lstats.inbound ?? {}).reduce((a, b) => a + b, 0) / (lstats.inboundMs / 1000)).toFixed(1) : null,
    feedPollsOnLeader: lstats.inbound?.["/feed"] ?? 0, inbound: lstats.inbound, inboundMs: lstats.inboundMs, failedCalls: lstats.failedCalls,
    // read-beat's question: whether the leader's member clock was suspect (lease-clock), and how many reads it sent
    // to a round for it.
    clock: lstats.clock ?? null },
  feedMembers: after.map((s) => ({ site: s.site, role: s.role, feed: s.feed })),
  ...(b034 ? { b034: { misses: missRows, missSummary: { n: missRows.length,
      compose: missRows.reduce((m, x) => ((m[x.compose ?? "none"] = (m[x.compose ?? "none"] ?? 0) + 1), m), {}),
      filled: missRows.filter((x) => x.filled).length, notFilled: missRows.filter((x) => x.filled === false).length }, kTimeline, kMax: Object.fromEntries([...new Set(kTimeline.map((x) => x[1]))].map((c) => [c, Math.max(0, ...kTimeline.filter((x) => x[1] === c).map((x) => x[2] ?? 0))])),
    secondAt: b034.secondAt, hostileAt: b034.hostileAt, killAt: opt["kill-shard-at"] ? Number(opt["kill-shard-at"]) : null } } : {}),
  ...(admissionSeen.length ? { admission: admissionSeen } : {}),
  ...(pushMarks.timed ? { pushTiming: { ...pushMarks, stages: Object.fromEntries(Object.entries(pushStages).map(([k, xs]) => [k, ps(xs)])) } } : {}),
  ...(mergeSteps.length ? { mergeSplit: mergeSplit(mergeSteps), mergeSteps } : {}),
  ...(plan.leases ? { leases: {
    overlap: { samples: overlaps.length, over0: overlaps.filter(([o]) => o > 0).length, over500: overlaps.filter(([o]) => o > 500).length,
      maxMs: overlaps.length ? Math.max(...overlaps.map(([o]) => o)) : null, p50: pct(overlaps.map(([o]) => o), 0.5),
      asksP50: pct(overlaps.map(([, n]) => n), 0.5) },
    // A grant is decided on its leader's own `at`, so the stamps alone never show two leaders' clock skew: the bound
    // on a real overlap adds the members' clock spread (clock.mjs's upper bound).
    overlapBoundMs: overlaps.length && clockSpread(clockSamples).upperMs != null
      ? Math.round(Math.max(...overlaps.map(([o]) => o)) + clockSpread(clockSamples).upperMs) : null,
    clock: { samples: clockSamples.length, ...clockSpread(clockSamples), raw: clockSamples.map((c) => [c.round, c.site, Math.round(c.offset), c.rtt, c.term, c.role,
      c.before == null ? null : Math.round(c.before), c.tick, c.lease]),
      // The gate line: each member's offset less the driver's (the mean of its before and after readings, or the
      // one that answered), so it is the member against Cloudflare's clock. `maxAbsOffsetRawMs` is against this orb.
      driver: driverClock, driverOffsetMs: driverMs(),
      maxAbsOffsetMs: clockSamples.length && driverMs() != null
        ? Math.round(Math.max(...clockSamples.map((c) => Math.abs(c.offset - driverMs())))) : null,
      maxAbsOffsetRawMs: clockSamples.length ? Math.round(Math.max(...clockSamples.map((c) => Math.abs(c.offset)))) : null },
  } } : {}),
  // Every plan, not only `openPrs`: B0-31's agents' test merges go through the same budget (m3fr4 wrote none).
  ...(opt.workflow ? { dispatch: dispatchLine(dispatchAcks, [...dispatchRuns.values()], t0, DISPATCH_POLL_MS, dispatchPolls) } : {}),
  testMerges: { ...budgetWindow(budgetBefore, budgetAfter, seconds), leader: lstats.testMerges ?? null,
    pollAnswers: codes["read:mergeable"] ?? {}, restPollAnswers: codes["read:mergeableRest"] ?? {},
    firstFor: opt["first-for"] ? Number(opt["first-for"]) : null },
};
writeRow(OUT, v);
log(JSON.stringify(v).slice(0, 4000));
// L0-12: the replay's tasks hold every requester's TASKS_OPEN_PER_REQUESTER (100) open slots, m3-ci's own included,
// so the row's stall and refusal parts after it could create no task (rows m22: 422 "The requester has too many open
// tasks", which the stall's lease acquires then read as "An App token must name its session"). Cancel them, ten at a
// time; cancelling a task releases its leases in the same command.
if (plan.leases) {
  const open = [...agents, ...probers].filter((a) => a.task);
  const answers = {};
  for (let b = 0; b < open.length; b += 10) {
    await Promise.all(open.slice(b, b + 10).map(async (a) => {
      const r = await api("POST", `${V1}/tasks/${a.task}/cancel`, app, {});
      answers[r.status] = (answers[r.status] ?? 0) + 1;
    }));
  }
  writeRow(OUT, { step: "tasks cancelled", n: open.length, answers });
  log(`tasks cancelled ${open.length}: ${JSON.stringify(answers)}`);
}
// `--workflow`: no runner takes the runs' label, so each would sit queued for QUEUE_LIMIT; cancel every run seen.
if (opt.workflow) {
  await pollRuns().catch(() => {});
  const ids = [...dispatchRuns.keys()];
  const answers = {};
  for (let b = 0; b < ids.length; b += 10) {
    await Promise.all(ids.slice(b, b + 10).map(async (id) => {
      const r = await api("POST", `/api/v3/repos/${R}/actions/runs/${id}/cancel`, admin, {});
      answers[r.status] = (answers[r.status] ?? 0) + 1;
    }));
  }
  writeRow(OUT, { step: "runs cancelled", n: ids.length, answers });
  log(`runs cancelled ${ids.length}: ${JSON.stringify(answers)}`);
}
// Pushes and polls cut off at the row's end can hold sockets open for minutes, which stalled `rows.sh` on B0-29 and
// B0-31 (`tests/am1/results/lm1-memory.md`, section 10): the row is written, so end here.
process.exit(0);
