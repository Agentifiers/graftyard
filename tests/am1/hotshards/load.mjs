// hot-shards' push driver on graft-m3c: real git pushes by m3-admin (Human class) to refs/heads/hot/<i> of one fresh
// repository through graft-m3c-edge, each branch with one push in flight, as tests/am1/b030/load.mjs pushes.
//
//   MODE=seed  PUSH_PER_S=1.6 node tests/am1/hotshards/load.mjs   # 400 KB packs until the log above the last Promoted
//                                                                  # passes SEED_BYTES (default 2.6e9) on every member
//   MODE=j9    PUSH_PER_S=4.17 SECONDS=5400 node tests/am1/hotshards/load.mjs   # journey 9: 10 to 70 KB, 5% 1 MiB
//
// Each blob is a fixed random base with 64 bytes replaced, so packs are incompressible inline and a promotion deltas
// them. One row per push in tests/am1/raw/hotshards/pushes.jsonl (at, ms, bytes, result) and a minute summary on
// stdout. The repository is $HOT_REPO (name) and $HOT_CHAIN (repo:<id>). Needs tests/am1/lf13/env-m3c.sh sourced.
import { createHash, randomBytes } from "node:crypto";
import { mkdirSync } from "node:fs";
import { deflateSync } from "node:zlib";
import { EDGE, log, member, row, sleep, tokens, voterNames } from "../hosted/lib.mjs";

const R = process.env.HOT_REPO;
const CHAIN = process.env.HOT_CHAIN;
const MODE = process.env.MODE ?? "j9";
// The seed stays under HOT_PUSHES (2/s), so its 900 KB packs stay inline (over INLINE_MAX_HOT a hot group stages them).
const PUSH_PER_S = Number(process.env.PUSH_PER_S ?? (MODE === "seed" ? 1.6 : 4.17));
const SECONDS = Number(process.env.SECONDS ?? 5400);
const SEED_BYTES = Number(process.env.SEED_BYTES ?? 2.6e9);
const BRANCHES = 500;
const INFLIGHT = Number(process.env.INFLIGHT ?? 8);
const OUT = "tests/am1/raw/hotshards/pushes.jsonl";
mkdirSync("tests/am1/raw/hotshards", { recursive: true });
if (!R || !CHAIN) throw new Error("HOT_REPO and HOT_CHAIN");

const size = () => {
  // 400 KB at 1.6/s is 0.64 MB/s of log, under the 0.8 MB/s B0-30 carried (tests/am1/results/b0-30.md, "Blocked on",
  // 7); 900 KB packs at that rate reset the leader's isolate.
  // SEED_PACK=lo-hi: sizes drawn evenly from the range, to build a tail of a given pack count and size.
  if (MODE === "seed") {
    const [lo, hi] = String(process.env.SEED_PACK ?? 400_000).split("-").map(Number);
    return hi ? lo + Math.floor(Math.random() * (hi - lo)) : lo;
  }
  return Math.random() < 0.05 ? 1 << 20 : 10_000 + Math.floor(Math.random() * 60_000);
};

// ---- git (tests/am1/b030/load.mjs) ----
const sha1 = (b) => createHash("sha1").update(b).digest();
const obj = (type, body) => sha1(Buffer.concat([Buffer.from(`${type} ${body.length}\0`), body]));
const TYPE = { commit: 1, tree: 2, blob: 3 };
function entryHeader(type, n) {
  const out = [];
  let b = (TYPE[type] << 4) | (n & 15);
  n >>= 4;
  while (n) { out.push(b | 0x80); b = n & 0x7f; n >>= 7; }
  out.push(b);
  return Buffer.from(out);
}
function onePack(path, bytes, parent, message) {
  const blob = obj("blob", bytes);
  const [dir, name] = path.split("/").length > 1 ? [path.split("/")[0], path.split("/").slice(1).join("/")] : [null, path];
  const leaf = Buffer.concat([Buffer.from(`100644 ${name}\0`), blob]);
  const objects = [["tree", leaf]];
  let root = obj("tree", leaf);
  if (dir) {
    const top = Buffer.concat([Buffer.from(`40000 ${dir}\0`), root]);
    objects.push(["tree", top]);
    root = obj("tree", top);
  }
  const at = Math.floor(Date.now() / 1000);
  const who = `hot <hot@graftyard.invalid> ${at} +0000`;
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
  Buffer.concat([pkt(`${old ?? ZERO} ${nu} ${ref}\0report-status agent=graftyard-hot\n`), Buffer.from("0000"), pack]);
function pushResult(text) {
  const lines = [];
  for (let i = 0; i + 4 <= text.length;) {
    const n = parseInt(text.slice(i, i + 4), 16);
    if (!/^[0-9a-f]{4}$/.test(text.slice(i, i + 4)) || (n > 0 && n < 4)) break;
    if (n === 0) { i += 4; continue; }
    lines.push(text.slice(i + 4, i + n));
    i += n;
  }
  const ng = lines.map((l) => l.match(/^ng (\S+) ([^\n]*)/)).find(Boolean);
  if (ng) return /stale|fetch first|non-fast-forward/.test(ng[2]) ? "stale" : `ng ${ng[2].slice(0, 50)}`;
  return lines.some((l) => l.startsWith("ok refs/")) ? "ok" : "error";
}
const auth = { authorization: `Basic ${Buffer.from(`x:${tokens.admin}`).toString("base64")}` };
async function git(path, body, headers) {
  try {
    const r = await fetch(`${EDGE}/${R}.git/${path}`, { method: "POST", body, headers: { ...auth, "user-agent": "git/2.56.0", ...headers }, signal: AbortSignal.timeout(180_000) });
    return { status: r.status, text: Buffer.from(await r.arrayBuffer()).toString("latin1") };
  } catch (e) { return { status: 0, text: String(e).slice(0, 80) }; }
}
async function tip(ref) {
  const r = await git("git-upload-pack", Buffer.concat([pkt("command=ls-refs\n"), Buffer.from("0001"), pkt(`ref-prefix ${ref}\n`), Buffer.from("0000")]),
    { "content-type": "application/x-git-upload-pack-request", "git-protocol": "version=2" });
  return r.text.match(new RegExp(`([0-9a-f]{40}) ${ref}\\n`))?.[1] ?? null;
}
const BASE = (() => { const out = []; for (let i = 0; out.length * 32 < (1 << 20) + 32; i++) out.push(createHash("sha256").update(`hot-base-${i}`).digest()); return Buffer.concat(out); })();
const tips = {};
const busy = new Set();
async function push(k) {
  let i = k % BRANCHES;
  while (busy.has(i)) i = (i + 1) % BRANCHES;
  busy.add(i);
  try {
    const ref = `refs/heads/hot/${i}`;
    if (!(i in tips)) tips[i] = await tip(ref);
    const n = size();
    const blob = Buffer.from(BASE.subarray(0, n));
    randomBytes(64).copy(blob, Math.floor(Math.random() * (n - 64)));
    const { pack, commit } = onePack(`hot/f${i}.bin`, blob, tips[i], `hot ${k}`);
    const t0 = Date.now();
    const r = await git("git-receive-pack", receiveBody(ref, tips[i], commit, pack), { "content-type": "application/x-git-receive-pack-request" });
    const ms = Date.now() - t0;
    const res = r.status === 200 ? pushResult(r.text) : `http ${r.status}`;
    if (res === "ok") tips[i] = commit;
    else if (res === "stale") tips[i] = await tip(ref);
    row(OUT, { at: new Date(t0).toISOString(), mode: MODE, ms, bytes: pack.length, res });
    return { res, ms };
  } finally { busy.delete(i); }
}

// During the seed, each member's archive cap is raised (test-faults `archiveCap`, per isolate) so promote-ahead
// (log above the last Promoted past max/32) and archiving hold off until the backlog is built; `MODE=uncap` clears it.
async function cap(max) {
  const names = await voterNames(CHAIN).catch(() => ({}));
  return Promise.all([0, 1, 2].map((s) => member(CHAIN, s, "/bench_seed", { archiveCap: max ? { max, range: 2 << 20 } : {} }, 25_000, names[s])
    .then((r) => r.v?.archiveCap ?? r.v).catch((e) => String(e).slice(0, 60))));
}
if (MODE === "uncap") {
  log(`hot uncap: ${JSON.stringify(await cap(null))}`);
  process.exit(0);
}

async function unpromoted() {
  const out = [];
  const names = await voterNames(CHAIN).catch(() => ({}));
  for (const s of [0, 1, 2]) {
    try { out.push((await member(CHAIN, s, "/bench_seed", { b030: { size: true } }, 25_000, names[s])).v?.unpromotedBytes ?? 0); }
    catch { out.push(0); }
  }
  return Math.min(...out);
}

log(`hot load: ${MODE} on ${R} (${CHAIN}), ${PUSH_PER_S}/s`);
const t0 = Date.now();
let due = 0, k = 0, inflight = 0, last = Date.now(), lastRow = Date.now(), lastCheck = 0;
const tally = {}, ms = [];
let stop = false;
while (!stop) {
  const now = Date.now();
  due = Math.min(due + PUSH_PER_S * Math.min(5, (now - last) / 1000), PUSH_PER_S * 5);
  last = now;
  // At most INFLIGHT pushes at once (default 8, as tests/am1/b030/load.mjs): 21 seed pushes of 900 KB in flight reset
  // the edge's isolate ("Cannot invoke closure from previous WASM instance"), and every push in it answered 500.
  while (due >= 1 && inflight < INFLIGHT) {
    due -= 1;
    inflight++;
    push(k++).then((r) => { tally[r.res] = (tally[r.res] ?? 0) + 1; ms.push(r.ms); })
      .catch((e) => { tally[`error ${String(e).slice(0, 30)}`] = (tally[`error ${String(e).slice(0, 30)}`] ?? 0) + 1; })
      .finally(() => inflight--);
  }
  if (now - lastRow >= 60_000) {
    lastRow = now;
    ms.sort((a, b) => a - b);
    const q = (p) => ms[Math.min(ms.length - 1, Math.floor(ms.length * p))];
    log(`hot ${MODE} ${((now - t0) / 1000).toFixed(0)} s: ${k} sent, inflight ${inflight}, ${JSON.stringify(tally)} p50 ${q(0.5)} p99 ${q(0.99)} max ${ms.at(-1)}`);
    for (const t of Object.keys(tally)) delete tally[t];
    ms.length = 0;
  }
  if (MODE === "seed" && now - lastCheck >= 120_000) {
    lastCheck = now;
    await cap(100e9);
    const u = await unpromoted();
    log(`hot seed: ${(u / 1e9).toFixed(3)} GB above the last Promoted on the least member`);
    if (u >= SEED_BYTES) stop = true;
  }
  if (MODE !== "seed" && now - t0 >= SECONDS * 1000) stop = true;
  await sleep(100);
}
while (inflight) await sleep(500);
log(`hot load ${MODE} ended: ${k} pushes`);
