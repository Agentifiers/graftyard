// B0-30's drain (operator, 2026-11-15): when load.mjs stops on simulated time at day 30, the pushes its schedule owed
// and had not sent (`pushDue` in its state, the backlog the slower push path built up) are pushed here, on the same
// group, at load.mjs's concurrency (8 in flight, one per branch), with load.mjs's pack shape, before the final
// readings. Dropping them would be a substitution against "pushes on". A push that fails is sent again, up to
// TRIES times, so every owed push is carried or counted as lost.
//
// Resumable: it reads and writes load.mjs's state (tests/am1/raw/b030-state.json), so it never runs beside load.mjs:
// it starts only once tests/am1/raw/b030-load.done exists. It sends nothing when tests/am1/raw/b030-STOP exists (a
// cost or watch stop), and stops at once if one appears. Rows (`kind: "drain"`, one a minute) go to
// tests/am1/raw/b030-load.jsonl; at the end it writes tests/am1/raw/b030-drain.done with what it carried and its wall
// time, and tests/am1/raw/b030-run.done, which ends the watcher.
//   node tests/am1/b030/drain.mjs
// B030_STATE, B030_REPO and B030_RAW move the state, the repository file and the raw directory (the local test's).
import { createHash, randomBytes } from "node:crypto";
import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { deflateSync } from "node:zlib";
import { EDGE, log, row, sleep, tokens } from "../hosted/lib.mjs";

const RAW = process.env.B030_RAW ?? "tests/am1/raw";
const REPO = JSON.parse(readFileSync(process.env.B030_REPO ?? `${RAW}/b030-repo.json`, "utf8"));
const R = REPO.name;
const STATE = process.env.B030_STATE ?? `${RAW}/b030-state.json`;
const STOP = `${RAW}/b030-STOP`;
const OUT = `${RAW}/b030-load.jsonl`;
const INFLIGHT = 8;
const TRIES = 3;
const BRANCHES = 500;
// load.mjs's defaults: 1.6 pushes a second at 18x carrying journey 9's 2.48 GB a simulated day.
const SPEED = 18, PUSH_PER_S = 1.6, DAY_S = 86_400 / SPEED;
const PUSH_BYTES = Math.round(((74.4 / 30) * 1e9) / (PUSH_PER_S * DAY_S));

while (!existsSync(`${RAW}/b030-load.done`)) await sleep(10_000);
const st = JSON.parse(readFileSync(STATE, "utf8"));
const save = () => { writeFileSync(`${STATE}.tmp`, JSON.stringify(st)); renameSync(`${STATE}.tmp`, STATE); };
const simMs = () => Date.parse("2026-11-10T00:00:00Z") + st.active * SPEED * 1000;

// ---- git, as load.mjs builds a push ----
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
    if (dir === null) return { objects };
    child = { mode: "40000", name: dir, id };
  }
}
function onePack(path, bytes, parent, message, at) {
  const { objects } = treeFor(path, obj("blob", bytes));
  const root = obj("tree", objects.at(-1)[1]);
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
const BASE = (() => { const out = []; for (let i = 0; out.length * 32 < PUSH_BYTES + 32; i++) out.push(createHash("sha256").update(`b030-base-${i}`).digest()); return Buffer.concat(out).subarray(0, PUSH_BYTES); })();
const busy = new Set();
const pushMs = [];
async function pushOn(i, k) {
  const ref = `refs/heads/b030/${i}`;
  const blob = Buffer.from(BASE);
  randomBytes(64).copy(blob, Math.floor(Math.random() * (blob.length - 64)));
  const parent = st.tips[i] ?? st.main;
  const { pack, commit } = onePack(`b030/${i}/f.bin`, blob, parent, `b030 ${k}`, Math.floor(simMs() / 1000));
  const t0 = Date.now();
  const r = await git("git-receive-pack", receiveBody(ref, st.tips[i] ?? null, commit, pack), { "content-type": "application/x-git-receive-pack-request" });
  pushMs.push(Date.now() - t0);
  const res = r.status === 200 ? pushResult(r.text) : `http ${r.status}`;
  if (res === "ok") { st.tips[i] = commit; st.pushes++; st.pushBytes += pack.length; }
  else if (res === "stale") st.tips[i] = await tip(ref);
  return res;
}
/** One owed push, sent until it lands or TRIES sends failed. */
async function owedPush(k) {
  let i = k % BRANCHES;
  while (busy.has(i)) i = (i + 1) % BRANCHES;
  busy.add(i);
  try {
    for (let t = 1; t <= TRIES; t++) {
      const res = await pushOn(i, k);
      note(`push ${res}`);
      if (res === "ok") return true;
      d.fails++;
    }
    return false;
  } finally { busy.delete(i); }
}

// ---- the drain ----
const tally = {};
const note = (k) => { tally[k] = (tally[k] ?? 0) + 1; };
st.drain ??= { started: new Date().toISOString(), owed: Math.floor(st.pushDue ?? 0), carried: 0, lost: 0, fails: 0, wallS: 0 };
const d = st.drain;
const t0 = Date.now(), wall0 = d.wallS;
log(`b030 drain: ${R}, ${Math.floor(st.pushDue ?? 0)} owed of ${d.owed}, ${INFLIGHT} in flight`);
let inflight = 0, lastRow = Date.now(), lastSave = Date.now();
while ((st.pushDue ?? 0) >= 1 && !existsSync(STOP)) {
  while (st.pushDue >= 1 && inflight < INFLIGHT) {
    st.pushDue -= 1;
    inflight++;
    owedPush(st.next++).then((ok) => { if (ok) d.carried++; else { d.lost++; st.pushFails++; } })
      .catch((e) => { d.lost++; note(`push error ${String(e).slice(0, 30)}`); }).finally(() => inflight--);
  }
  const now = Date.now();
  d.wallS = wall0 + (now - t0) / 1000;
  if (now - lastSave > 15_000) { save(); lastSave = now; }
  if (now - lastRow > 60_000) {
    lastRow = now;
    pushMs.sort((a, b) => a - b);
    row(OUT, { kind: "drain", at: new Date().toISOString(), owedLeft: Math.floor(st.pushDue), carried: d.carried, lost: d.lost, fails: d.fails, pushes: st.pushes,
      pushMs: pushMs.length ? { n: pushMs.length, p50: pushMs[pushMs.length >> 1], max: pushMs.at(-1) } : null, tally });
    for (const k of Object.keys(tally)) delete tally[k];
    pushMs.length = 0;
  }
  await sleep(250);
}
while (inflight) await sleep(500);
d.wallS = wall0 + (Date.now() - t0) / 1000;
d.ended = new Date().toISOString();
d.stopped = existsSync(STOP) ? readFileSync(STOP, "utf8").trim() : null;
save();
log(`b030 drain ended: ${JSON.stringify(d)}`);
writeFileSync(`${RAW}/b030-drain.done`, JSON.stringify({ ...d, owedLeft: Math.floor(st.pushDue ?? 0), pushes: st.pushes }) + "\n");
writeFileSync(`${RAW}/b030-run.done`, new Date().toISOString() + "\n");
